//! Standalone GPU encoder probe. Build/run in release on Windows for timings.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = probe::run() {
        eprintln!("ffmpeg_probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod probe {
    use host_core::ffmpeg_abi::{ffmpeg_dir, version_parts, FFMPEG_DIR_ENV};
    use host_win::ffmpeg::{FfEncoder, FfLib, FfOpen, FfPacket, HwPath, InputSurface};
    use std::{
        error::Error,
        fmt::Write as _,
        fs::File,
        io::{BufWriter, Write},
        path::PathBuf,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };
    use windows::{
        core::Interface,
        Win32::{
            Foundation::HMODULE,
            Graphics::{
                Direct3D::{
                    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
                },
                Direct3D11::*,
                Dxgi::{
                    Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
                    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
                    DXGI_ERROR_NOT_FOUND,
                },
            },
            System::Threading::{GetCurrentProcess, GetProcessHandleCount},
        },
    };

    type Result<T> = std::result::Result<T, Box<dyn Error>>;
    type Options = Vec<(String, String, bool)>;

    struct Args {
        list: bool,
        dir: PathBuf,
        adapter: Option<usize>,
        backend: Option<String>,
        codec: String,
        width: u32,
        height: u32,
        fps: u32,
        kbps: u32,
        frames: u32,
        pattern: String,
        idr_every: u32,
        rate_step: Option<(u32, u32)>,
        roi_test: bool,
        cycles: Option<u32>,
        out: Option<PathBuf>,
        options: Options,
    }
    impl Args {
        fn parse() -> Result<Option<Self>> {
            let exe = std::env::current_exe()?;
            let dir = ffmpeg_dir(
                exe.parent().ok_or("executable has no parent")?,
                std::env::var(FFMPEG_DIR_ENV).ok().as_deref(),
            );
            let mut out = Self {
                list: false,
                dir,
                adapter: None,
                backend: None,
                codec: "hevc".into(),
                width: 1920,
                height: 1080,
                fps: 60,
                kbps: 10000,
                frames: 600,
                pattern: "noise".into(),
                idr_every: 0,
                rate_step: None,
                roi_test: false,
                cycles: None,
                out: None,
                options: vec![],
            };
            let mut args = std::env::args().skip(1);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--help" | "-h" => {
                        println!("ffmpeg_probe [--list] [--dir PATH] [--adapter N] [--backend nvenc|amf|qsv] [--codec hevc|h264] [--size WxH] [--fps 60] [--kbps 10000] [--frames 600] [--pattern noise|bar] [--idr-every N] [--rate-step KBPS@FRAME] [--roi-test] [--cycles N] [--out FILE] [--opt NAME=VALUE]\nFrame numbers/PTS are zero-based. --roi-test uses 131 frames; --cycles uses 30 frames per open. RS_FFMPEG_LOG=verbose enables FFmpeg verbose messages.");
                        return Ok(None);
                    }
                    "--list" => out.list = true,
                    "--roi-test" => out.roi_test = true,
                    _ => {
                        let value = args
                            .next()
                            .ok_or_else(|| format!("missing value for {arg}"))?;
                        match arg.as_str() {
                            "--dir" => out.dir = value.into(),
                            "--adapter" => out.adapter = Some(value.parse()?),
                            "--backend" => out.backend = Some(value),
                            "--codec" => out.codec = value,
                            "--size" => {
                                let (w, h) = value.split_once('x').ok_or("size must be WxH")?;
                                out.width = w.parse()?;
                                out.height = h.parse()?;
                            }
                            "--fps" => out.fps = value.parse()?,
                            "--kbps" => out.kbps = value.parse()?,
                            "--frames" => out.frames = value.parse()?,
                            "--pattern" => out.pattern = value,
                            "--idr-every" => out.idr_every = value.parse()?,
                            "--rate-step" => {
                                let (kbps, frame) = value
                                    .split_once('@')
                                    .ok_or("rate-step must be KBPS@FRAME")?;
                                out.rate_step = Some((kbps.parse()?, frame.parse()?));
                            }
                            "--cycles" => out.cycles = Some(value.parse()?),
                            "--out" => out.out = Some(value.into()),
                            "--opt" => {
                                let (name, val) =
                                    value.split_once('=').ok_or("opt must be NAME=VALUE")?;
                                if name.is_empty() || value.contains('\0') {
                                    return Err("invalid --opt".into());
                                }
                                out.options.push((name.into(), val.into(), false));
                            }
                            _ => return Err(format!("unknown argument: {arg}").into()),
                        }
                    }
                }
            }
            if out.width == 0
                || out.height == 0
                || out.width > 16384
                || out.height > 16384
                || !out.width.is_multiple_of(2)
                || !out.height.is_multiple_of(2)
            {
                return Err("size must be positive even dimensions up to 16384".into());
            }
            if out.fps == 0
                || out.fps > 240
                || out.frames == 0
                || out.kbps == 0
                || out.kbps > 1_000_000
                || out.cycles == Some(0)
            {
                return Err("invalid fps/frames/kbps/cycles".into());
            }
            if !matches!(out.codec.as_str(), "hevc" | "h264")
                || !matches!(out.pattern.as_str(), "noise" | "bar")
                || out
                    .backend
                    .as_deref()
                    .is_some_and(|b| !matches!(b, "nvenc" | "amf" | "qsv"))
            {
                return Err("invalid codec/pattern/backend".into());
            }
            if out.roi_test && (out.cycles.is_some() || out.rate_step.is_some())
                || out.cycles.is_some() && out.rate_step.is_some()
            {
                return Err("roi-test, rate-step and cycles are separate experiments".into());
            }
            if out
                .rate_step
                .is_some_and(|(rate, frame)| rate == 0 || rate > 1_000_000 || frame >= out.frames)
            {
                return Err("invalid rate-step rate or frame".into());
            }
            Ok(Some(out))
        }
    }

    // Minimal stderr subscriber using the existing tracing dependency. It also
    // exposes optional AVOption rejections without creating a log directory.
    struct Stderr;
    impl tracing::Subscriber for Stderr {
        fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
            *m.level() <= tracing::Level::DEBUG
                || std::env::var("RS_FFMPEG_LOG").as_deref() == Ok("verbose")
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    let _ = write!(self.0, " {}={value:?}", field.name());
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
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    struct Adapter {
        adapter: IDXGIAdapter1,
        vendor: u32,
        output: bool,
    }
    fn adapters() -> Result<Vec<Adapter>> {
        // SAFETY: requests an owned DXGI factory with its proper interface IID.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let mut found = Vec::new();
        for index in 0.. {
            // SAFETY: live factory; EnumAdapters1 returns owned COM references.
            let adapter = match unsafe { factory.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            // SAFETY: live adapter and owned returned descriptor/output.
            let (desc, output) = unsafe { (adapter.GetDesc1()?, adapter.EnumOutputs(0)) };
            let output = match output {
                Ok(_) => true,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => false,
                Err(e) => return Err(e.into()),
            };
            let end = desc
                .Description
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            let luid =
                (i64::from(desc.AdapterLuid.HighPart) << 32) | i64::from(desc.AdapterLuid.LowPart);
            println!("adapter={index} description={name:?} VendorId={:#06x} LUID={luid:#018x} output={output}", desc.VendorId);
            found.push(Adapter {
                adapter,
                vendor: desc.VendorId,
                output,
            });
        }
        Ok(found)
    }
    fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
        let adapter: IDXGIAdapter = adapter.cast()?;
        let create = |flags| -> Result<_> {
            let mut device = None;
            let mut context = None;
            // SAFETY: live adapter, supported feature-level array and writable
            // COM output slots; no software driver module is supplied.
            unsafe {
                D3D11CreateDevice(
                    Some(&adapter),
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    flags,
                    Some(&[
                        D3D_FEATURE_LEVEL_11_1,
                        D3D_FEATURE_LEVEL_11_0,
                        D3D_FEATURE_LEVEL_10_1,
                        D3D_FEATURE_LEVEL_10_0,
                    ]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
            Ok((
                device.ok_or("missing D3D11 device")?,
                context.ok_or("missing D3D11 context")?,
            ))
        };
        let (device, context) = create(
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        )
        .or_else(|error| {
            eprintln!("VIDEO_SUPPORT unavailable ({error}); retrying BGRA_SUPPORT");
            create(D3D11_CREATE_DEVICE_BGRA_SUPPORT)
        })?;
        let multithread: ID3D11Multithread = device.cast()?;
        // SAFETY: live interface; protects the shared immediate context for QSV.
        unsafe {
            let _ = multithread.SetMultithreadProtected(true);
        }
        Ok((device, context))
    }
    fn handle_count() -> Result<u32> {
        let mut count = 0;
        // SAFETY: current-process pseudo handle needs no CloseHandle; output lives through call.
        unsafe {
            GetProcessHandleCount(GetCurrentProcess(), &mut count)?;
        }
        Ok(count)
    }

    fn rate_options(backend: &str, kbps: u32, fps: u32) -> Options {
        let bps = u64::from(kbps) * 1000;
        let mut out = vec![
            (
                "b".into(),
                (bps - u64::from(backend == "qsv")).to_string(),
                true,
            ),
            ("maxrate".into(), bps.to_string(), true),
        ];
        if backend != "qsv" {
            out.push((
                "bufsize".into(),
                bps.div_ceil(u64::from(fps)).to_string(),
                true,
            ));
        }
        out
    }
    fn options(args: &Args, backend: &str) -> Options {
        // 正式编码器以 `encode_model::ffmpeg_options` 为准。
        // Standalone snapshot of the encoder option table, including required/optional flags;
        // QSV uses b=maxrate-1 and leaves bufsize to the driver.
        let mut out = vec![
            (
                "pixel_format".into(),
                if backend == "qsv" { "qsv" } else { "d3d11" }.into(),
                true,
            ),
            (
                "video_size".into(),
                format!("{}x{}", args.width, args.height),
                true,
            ),
            ("time_base".into(), format!("1/{}", args.fps), true),
        ];
        out.extend(rate_options(backend, args.kbps, args.fps));
        let common = [
            ("bf", "0", true),
            ("g", "2147483647", true),
            ("keyint_min", "2147483647", false),
            ("flags", "+cgop+low_delay", false),
            ("color_range", "tv", true),
            ("colorspace", "bt709", true),
            ("color_primaries", "bt709", true),
            ("color_trc", "bt709", true),
        ];
        out.extend(common.into_iter().map(|(k, v, r)| (k.into(), v.into(), r)));
        let extra: &[(&str, &str, bool)] = match backend {
            "nvenc" => &[
                ("preset", "p1", true),
                ("tune", "ull", true),
                ("rc", "cbr", true),
                ("zerolatency", "1", true),
                ("delay", "0", true),
                ("forced-idr", "1", true),
            ],
            "amf" => &[
                ("usage", "ultralowlatency", true),
                ("quality", "speed", true),
                ("rc", "cbr", true),
                ("latency", "1", false),
                ("async_depth", "1", false),
                ("forced_idr", "1", true),
                ("filler_data", "0", false),
            ],
            _ => &[
                ("preset", "veryfast", true),
                ("forced_idr", "1", true),
                ("async_depth", "1", true),
                ("low_delay_brc", "1", false),
                ("low_power", "1", false),
                ("scenario", "displayremoting", false),
            ],
        };
        out.extend(extra.iter().map(|&(k, v, r)| (k.into(), v.into(), r)));
        let specific: &[(&str, &str)] = match (backend, args.codec.as_str()) {
            ("amf", "h264") => &[("frame_skipping", "0")],
            ("amf", _) => &[
                ("skip_frame", "0"),
                ("gops_per_idr", "1"),
                ("header_insertion_mode", "idr"),
            ],
            ("qsv", "h264") => &[("recovery_point_sei", "0"), ("pic_timing_sei", "0")],
            _ => &[],
        };
        out.extend(specific.iter().map(|&(k, v)| (k.into(), v.into(), false)));
        out.push((
            "profile".into(),
            if args.codec == "hevc" { "main" } else { "high" }.into(),
            false,
        ));
        out.extend(args.options.iter().cloned());
        out
    }
    fn borrowed(options: &Options) -> Vec<(&str, &str, bool)> {
        options
            .iter()
            .map(|(k, v, r)| (k.as_str(), v.as_str(), *r))
            .collect()
    }

    struct Pattern {
        staging: ID3D11Texture2D,
        context: ID3D11DeviceContext,
        width: u32,
        height: u32,
        noise: bool,
    }
    impl Pattern {
        fn new(device: &ID3D11Device, context: &ID3D11DeviceContext, args: &Args) -> Result<Self> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: args.width,
                Height: args.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            // SAFETY: valid NV12 staging description and live COM output slot.
            unsafe {
                device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            }
            Ok(Self {
                staging: staging.ok_or("missing staging texture")?,
                context: context.clone(),
                width: args.width,
                height: args.height,
                noise: args.pattern == "noise",
            })
        }
        fn draw(&self, surface: &InputSurface, frame: u32) -> Result<()> {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: staging texture has CPU_ACCESS_WRITE and is currently unmapped.
            unsafe {
                self.context
                    .Map(&self.staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))?;
            }
            // No fallible work/panics between Map and Unmap. Each row writes only
            // Width bytes; NV12 UV begins at RowPitch * Height, not Width * Height.
            if mapped.pData.is_null() || mapped.RowPitch < self.width {
                // SAFETY: balances the successful Map even for an invalid result.
                unsafe {
                    self.context.Unmap(&self.staging, 0);
                }
                return Err("invalid NV12 mapped row pitch/data".into());
            }
            let pitch = mapped.RowPitch as usize;
            let bar = (u64::from(frame) * 13 % u64::from(self.width)) as u32;
            // SAFETY: D3D11 guarantees mapped Y/UV rows for this NV12 texture.
            // Writes remain inside the visible width and allocated plane heights.
            unsafe {
                let base = mapped.pData.cast::<u8>();
                for y in 0..self.height {
                    let row = base.add(y as usize * pitch);
                    for block in (0..self.width).step_by(64) {
                        // A different seed per 64x64 tile and frame; random pixels
                        // inside the tile keep the requested bitrate under load.
                        let mut rng = frame.wrapping_add(1).wrapping_mul(747796405)
                            ^ (y / 64).wrapping_mul(2891336453)
                            ^ block.wrapping_mul(277803737)
                            ^ y;
                        for x in block..(block + 64).min(self.width) {
                            rng ^= rng << 13;
                            rng ^= rng >> 17;
                            rng ^= rng << 5;
                            let gray = if self.noise {
                                16 + (rng % 220) as u8
                            } else {
                                32 + ((x / 64 + y / 64) % 2 * 96) as u8
                            };
                            row.add(x as usize)
                                .write(if x >= bar && x < bar.saturating_add(8) {
                                    235
                                } else {
                                    gray
                                });
                        }
                    }
                }
                let uv = base.add(pitch * self.height as usize);
                for y in 0..self.height / 2 {
                    ptr_fill(uv.add(y as usize * pitch), self.width as usize);
                }
                self.context.Unmap(&self.staging, 0);
                let region = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: self.width,
                    bottom: self.height,
                    back: 1,
                };
                self.context.CopySubresourceRegion(
                    &surface.texture,
                    surface.array_index,
                    0,
                    0,
                    0,
                    &self.staging,
                    0,
                    Some(&region),
                );
            }
            Ok(())
        }
    }
    unsafe fn ptr_fill(row: *mut u8, width: usize) {
        // SAFETY: caller supplies a valid mapped UV row with at least width bytes.
        unsafe {
            std::ptr::write_bytes(row, 128, width);
        }
    }

    #[derive(Default)]
    struct FrameOutput {
        bytes: u64,
        packets: u32,
        keyframe: bool,
    }
    struct Stats {
        frames: u32,
        packets: u64,
        bytes: u64,
        keys: u64,
        first_key: Option<bool>,
        times: Vec<u64>,
        by_pts: Vec<FrameOutput>,
        out: Option<BufWriter<File>>,
        window_start: Instant,
        window_bytes: u64,
        window_keys: u64,
        window_times: Vec<u64>,
    }
    impl Stats {
        fn new(frames: u32, out: Option<&PathBuf>) -> Result<Self> {
            Ok(Self {
                frames: 0,
                packets: 0,
                bytes: 0,
                keys: 0,
                first_key: None,
                times: Vec::new(),
                by_pts: (0..frames).map(|_| FrameOutput::default()).collect(),
                out: out.map(File::create).transpose()?.map(BufWriter::new),
                window_start: Instant::now(),
                window_bytes: 0,
                window_keys: 0,
                window_times: Vec::new(),
            })
        }
        fn record(&mut self, elapsed: u64, packets: Vec<FfPacket>) -> Result<()> {
            self.frames += 1;
            self.times.push(elapsed);
            self.window_times.push(elapsed);
            for packet in packets {
                let bytes = packet.data.len() as u64;
                if let Some(out) = self.out.as_mut() {
                    out.write_all(&packet.data)?;
                }
                self.first_key.get_or_insert(packet.keyframe);
                self.packets += 1;
                self.bytes += bytes;
                self.keys += u64::from(packet.keyframe);
                self.window_bytes += bytes;
                self.window_keys += u64::from(packet.keyframe);
                if let Ok(pts) = usize::try_from(packet.pts) {
                    if let Some(frame) = self.by_pts.get_mut(pts) {
                        frame.bytes += bytes;
                        frame.packets += 1;
                        frame.keyframe |= packet.keyframe;
                    }
                }
            }
            let duration = self.window_start.elapsed();
            if duration >= Duration::from_secs(1) {
                let (p50, p95) = percentiles(&mut self.window_times);
                println!(
                    "frames={} out_kbps={:.1} keyframes={} enc_p50_us={p50} enc_p95_us={p95}",
                    self.frames,
                    self.window_bytes as f64 * 8.0 / 1000.0 / duration.as_secs_f64(),
                    self.window_keys
                );
                self.window_start = Instant::now();
                self.window_bytes = 0;
                self.window_keys = 0;
                self.window_times.clear();
            }
            Ok(())
        }
        fn finish(&mut self, args: &Args, duration: Duration) -> Result<()> {
            if let Some(out) = self.out.as_mut() {
                out.flush()?;
            }
            let (p50, p95) = percentiles(&mut self.times);
            println!("total frames={} packets={} bytes={} keyframes={} out_kbps={:.1} enc_p50_us={p50} enc_p95_us={p95} first_packet_keyframe={:?} missing_pts={}", self.frames, self.packets, self.bytes, self.keys, self.bytes as f64 * 8.0 / 1000.0 / duration.as_secs_f64(), self.first_key, self.by_pts.iter().filter(|p| p.packets == 0).count());
            if let Some((_, step)) = args.rate_step {
                println!(
                    "rate-step frame={step} keyframe={:?}",
                    self.by_pts
                        .get(step as usize)
                        .filter(|p| p.packets > 0)
                        .map(|p| p.keyframe)
                );
                for (label, begin, end) in [
                    ("before", step.saturating_sub(30), step),
                    ("after", step, step.saturating_add(30).min(self.frames)),
                ] {
                    for start in (begin..end).step_by(10) {
                        let stop = (start + 10).min(end);
                        let slice = &self.by_pts[start as usize..stop as usize];
                        let bytes: u64 = slice.iter().map(|p| p.bytes).sum();
                        println!("rate-step {label} frames={start}..{} out_kbps={:.1} received_frames={}/{}", stop - 1, bytes as f64 * 8.0 * f64::from(args.fps) / f64::from(stop - start) / 1000.0, slice.iter().filter(|p| p.packets > 0).count(), stop - start);
                    }
                }
            }
            if args.roi_test {
                let normal = &self.by_pts[129];
                let roi = &self.by_pts[130];
                if normal.packets == 0 || roi.packets == 0 || normal.bytes == 0 {
                    println!(
                        "roi normal_bytes={} roi_bytes={} ratio=unavailable (missing/empty packet)",
                        normal.bytes, roi.bytes
                    );
                } else {
                    println!(
                        "roi normal_bytes={} roi_bytes={} ratio={:.3}",
                        normal.bytes,
                        roi.bytes,
                        roi.bytes as f64 / normal.bytes as f64
                    );
                }
            }
            Ok(())
        }
    }
    fn percentiles(values: &mut [u64]) -> (u64, u64) {
        if values.is_empty() {
            return (0, 0);
        }
        values.sort_unstable();
        (
            values[(values.len() * 50).div_ceil(100) - 1],
            values[(values.len() * 95).div_ceil(100) - 1],
        )
    }
    fn pace(start: Instant, frame: u32, fps: u32) {
        let deadline = start + Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            // Leave the last Windows timer quantum to yield_now. Encoding
            // measurements below exclude all pacing and texture generation.
            if remaining > Duration::from_millis(17) {
                thread::sleep(remaining - Duration::from_millis(16));
            } else {
                thread::yield_now();
            }
        }
    }
    fn open(
        lib: &Arc<FfLib>,
        args: &Args,
        backend: &str,
        device: &ID3D11Device,
        options: &Options,
    ) -> Result<FfEncoder> {
        let name = format!("{}_{}", args.codec, backend);
        Ok(FfEncoder::open(
            lib,
            &FfOpen {
                codec_name: &name,
                hw: if backend == "qsv" {
                    HwPath::Qsv
                } else {
                    HwPath::D3d11
                },
                width: args.width,
                height: args.height,
                device,
                options: &borrowed(options),
            },
        )?)
    }
    pub fn run() -> Result<()> {
        let Some(args) = Args::parse()? else {
            return Ok(());
        };
        tracing::subscriber::set_global_default(Stderr)?;
        let lib = FfLib::load(&args.dir)?;
        let versions = lib.versions();
        let (cm, cn, cp) = version_parts(versions.avcodec);
        let (um, un, up) = version_parts(versions.avutil);
        println!("FFmpeg dir={} avcodec={cm}.{cn}.{cp} avutil={um}.{un}.{up}\nlicense={}\nconfiguration={}", versions.dir.display(), versions.license, versions.configuration.chars().take(300).collect::<String>());
        for name in [
            "hevc_nvenc",
            "h264_nvenc",
            "hevc_amf",
            "h264_amf",
            "hevc_qsv",
            "h264_qsv",
        ] {
            println!(
                "{name}: {}",
                if lib.has_encoder(name) { "yes" } else { "no" }
            );
        }
        let adapters = adapters()?;
        if args.list {
            return Ok(());
        }
        let index = args
            .adapter
            .or_else(|| adapters.iter().position(|a| a.output))
            .ok_or("no adapter with output; use --adapter")?;
        let adapter = adapters.get(index).ok_or("adapter index out of range")?;
        let backend = match args.backend.as_deref() {
            Some(backend) => backend,
            None => match adapter.vendor {
                0x10de => "nvenc",
                0x1002 => "amf",
                0x8086 => "qsv",
                _ => return Err("unknown vendor; specify --backend".into()),
            },
        };
        let (device, context) = create_device(&adapter.adapter)?;
        let pattern = Pattern::new(&device, &context, &args)?;
        let options = options(&args, backend);
        if let Some(cycles) = args.cycles {
            let before = handle_count()?;
            let mut stats = Stats::new(30, args.out.as_ref())?;
            let run_start = Instant::now();
            for cycle in 0..cycles {
                let mut encoder = open(&lib, &args, backend, &device, &options)?;
                for frame in 0..30 {
                    pattern.draw(encoder.input(), frame)?;
                    let start = Instant::now();
                    let packets = encoder.encode(i64::from(frame), frame == 0, None)?;
                    stats.record(start.elapsed().as_micros() as u64, packets)?;
                }
                drop(encoder);
                println!("cycle={}/{} handles={}", cycle + 1, cycles, handle_count()?);
            }
            stats.finish(&args, run_start.elapsed())?;
            let after = handle_count()?;
            println!(
                "cycles={cycles} handles_before={before} handles_after={after} delta={}",
                i64::from(after) - i64::from(before)
            );
            return Ok(());
        }
        let mut encoder = open(&lib, &args, backend, &device, &options)?;
        println!(
            "opened {} adapter={index} {}x{} fps={}",
            encoder.codec_name(),
            args.width,
            args.height,
            args.fps
        );
        let frames = if args.roi_test { 131 } else { args.frames };
        let mut stats = Stats::new(frames, args.out.as_ref())?;
        let start = Instant::now();
        for frame in 0..frames {
            pace(start, frame, args.fps);
            if let Some((kbps, step)) = args.rate_step {
                if frame == step {
                    encoder.set_options(&borrowed(&rate_options(backend, kbps, args.fps)))?;
                    println!("rate-step submitted frame={frame} kbps={kbps}");
                }
            }
            // 0..119 move; 120..129 repeat one image; 130 adds full-frame ROI.
            pattern.draw(
                encoder.input(),
                if args.roi_test { frame.min(119) } else { frame },
            )?;
            let idr = frame == 0 || args.idr_every != 0 && frame % args.idr_every == 0;
            let roi = (args.roi_test && frame == 130).then_some((-8, 51));
            let encode_start = Instant::now();
            let packets = encoder.encode(i64::from(frame), idr, roi)?;
            let elapsed = encode_start.elapsed().as_micros() as u64;
            stats.record(elapsed, packets)?;
        }
        stats.finish(&args, start.elapsed())?;
        drop(encoder);
        Ok(())
    }
}
