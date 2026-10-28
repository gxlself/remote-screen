//! Independent ScreenCaptureKit audio probe. Stdout is one JSON object per run.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("audio_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::fs::File;
    use std::io::{self, BufWriter, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use host_core::platform::{AudioCaptureConfig, AudioChunk, PlatformError};
    use host_mac::audio::MacAudioCapture;
    use rs_audio::codec::{AudioEncoder, EncoderConfig};
    use rs_audio::framer::Framer;
    use rsp_core::clock::unix_micros;
    use serde_json::{json, Value};
    use tokio::process::Command;
    use tokio::sync::mpsc;

    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u8 = 2;
    const BEEP_THRESHOLD: f32 = 0.031_622_78; // -30 dBFS.

    struct ProbeError {
        code: &'static str,
        message: String,
    }

    impl ProbeError {
        fn new(code: &'static str, message: impl Into<String>) -> Self {
            Self {
                code,
                message: message.into(),
            }
        }
    }

    impl From<PlatformError> for ProbeError {
        fn from(error: PlatformError) -> Self {
            let code = match &error {
                PlatformError::PermissionDenied(_) => "PermissionDenied",
                PlatformError::Unsupported => "Unsupported",
                PlatformError::Platform(_) | PlatformError::Io(_) => "Platform",
            };
            Self::new(code, error.to_string())
        }
    }

    impl From<io::Error> for ProbeError {
        fn from(error: io::Error) -> Self {
            Self::new("Io", error.to_string())
        }
    }

    impl From<rs_audio::AudioError> for ProbeError {
        fn from(error: rs_audio::AudioError) -> Self {
            Self::new("AudioEncode", error.to_string())
        }
    }

    struct Args {
        seconds: u64,
        out: PathBuf,
        encode: bool,
        json: bool,
        beep: bool,
    }

    fn parse_args() -> Result<Args, ProbeError> {
        let mut args = Args {
            seconds: 5,
            out: "target/audio-probe.wav".into(),
            encode: false,
            json: false,
            beep: false,
        };
        let mut values = std::env::args().skip(1);
        while let Some(flag) = values.next() {
            match flag.as_str() {
                "--seconds" => {
                    args.seconds = values
                        .next()
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(|| {
                            ProbeError::new("InvalidArguments", "--seconds requires N")
                        })?;
                }
                "--out" => {
                    args.out = values.next().map(PathBuf::from).ok_or_else(|| {
                        ProbeError::new("InvalidArguments", "--out requires a path")
                    })?;
                }
                "--encode" => args.encode = true,
                "--json" => args.json = true,
                "--beep" => args.beep = true,
                "--help" | "-h" => {
                    println!("usage: audio_probe --seconds 5 --out target/audio-probe.wav [--encode] [--json] [--beep]");
                    std::process::exit(0);
                }
                _ => {
                    return Err(ProbeError::new(
                        "InvalidArguments",
                        format!("unknown argument: {flag}"),
                    ));
                }
            }
        }
        if args.seconds == 0 || args.seconds > 300 || (args.beep && args.seconds < 2) {
            return Err(ProbeError::new(
                "InvalidArguments",
                "--seconds must be 1..300 (at least 2 with --beep)",
            ));
        }
        Ok(args)
    }

    #[derive(Clone, Copy)]
    struct BeepMark {
        called: Instant,
        unix_us: u64,
    }

    #[derive(Default)]
    struct Measurements {
        chunks: u64,
        frames: u64,
        first_ts_us: Option<u64>,
        previous_ts_us: Option<u64>,
        last_frames: u32,
        max_ts_gap_us: u64,
        ts_monotonic: bool,
        peak: f32,
        sum_squares: f64,
        sample_count: u64,
        first_chunk_ms: Option<f64>,
        beep_capture_latency_ms: Option<f64>,
        beep_sample_ts_us: Option<u64>,
    }

    impl Measurements {
        fn push(
            &mut self,
            chunk: &AudioChunk,
            received: Instant,
            started: Instant,
            beep: Option<BeepMark>,
        ) -> Result<(), ProbeError> {
            if chunk.channels != CHANNELS
                || chunk.frames == 0
                || chunk.samples.len() != chunk.frames as usize * usize::from(CHANNELS)
            {
                return Err(ProbeError::new(
                    "InvalidChunk",
                    "expected interleaved stereo PCM",
                ));
            }
            self.chunks += 1;
            self.frames += u64::from(chunk.frames);
            self.last_frames = chunk.frames;
            self.first_chunk_ms
                .get_or_insert_with(|| received.duration_since(started).as_secs_f64() * 1000.0);
            if let Some(previous) = self.previous_ts_us {
                self.ts_monotonic &= chunk.capture_ts_us > previous;
                self.max_ts_gap_us = self
                    .max_ts_gap_us
                    .max(chunk.capture_ts_us.saturating_sub(previous));
            } else {
                self.first_ts_us = Some(chunk.capture_ts_us);
                self.ts_monotonic = true;
            }
            self.previous_ts_us = Some(chunk.capture_ts_us);
            for (index, &sample) in chunk.samples.iter().enumerate() {
                let sample = sanitize(sample);
                self.peak = self.peak.max(sample.abs());
                self.sum_squares += f64::from(sample).powi(2);
                self.sample_count += 1;
                if let Some(mark) = beep.filter(|_| self.beep_capture_latency_ms.is_none()) {
                    let offset_us =
                        (index / usize::from(CHANNELS)) as u64 * 1_000_000 / u64::from(SAMPLE_RATE);
                    let sample_ts = chunk.capture_ts_us.saturating_add(offset_us);
                    if sample_ts >= mark.unix_us && sample.abs() > BEEP_THRESHOLD {
                        // Invocation -> arrival of the block containing the first
                        // threshold crossing. Includes afplay launch and block
                        // quantization; this is intentionally a coarse measure.
                        self.beep_capture_latency_ms =
                            Some(received.duration_since(mark.called).as_secs_f64() * 1000.0);
                        self.beep_sample_ts_us = Some(sample_ts);
                    }
                }
            }
            Ok(())
        }

        fn measured_rate_hz(&self) -> f64 {
            match (self.first_ts_us, self.previous_ts_us) {
                (Some(first), Some(last)) if last > first => {
                    // Both timestamps point at block starts. Exclude the last
                    // block from the sample count for the same time interval.
                    (self.frames - u64::from(self.last_frames)) as f64 * 1_000_000.0
                        / (last - first) as f64
                }
                _ => 0.0,
            }
        }

        fn json(&self, dropped: u64) -> Value {
            let rms = if self.sample_count > 0 {
                (self.sum_squares / self.sample_count as f64).sqrt()
            } else {
                0.0
            };
            json!({
                "chunks": self.chunks,
                "frames": self.frames,
                "measured_rate_hz": self.measured_rate_hz(),
                "channels": CHANNELS,
                "max_ts_gap_us": self.max_ts_gap_us,
                "ts_monotonic": self.ts_monotonic,
                "peak_dbfs": dbfs(f64::from(self.peak)),
                "rms_dbfs": dbfs(rms),
                "dropped": dropped,
                "first_chunk_ms": self.first_chunk_ms,
            })
        }
    }

    fn sanitize(sample: f32) -> f32 {
        if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    fn dbfs(amplitude: f64) -> f64 {
        // JSON cannot represent -infinity. A finite silence floor also lets
        // callers directly test peak_dbfs < -70 for digital silence.
        20.0 * amplitude.max(1e-8).log10()
    }

    fn wav_header(data_len: u32) -> [u8; 44] {
        let mut header = [0_u8; 44];
        header[..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&16_u32.to_le_bytes());
        header[20..22].copy_from_slice(&1_u16.to_le_bytes());
        header[22..24].copy_from_slice(&u16::from(CHANNELS).to_le_bytes());
        header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
        header[28..32].copy_from_slice(&(SAMPLE_RATE * u32::from(CHANNELS) * 2).to_le_bytes());
        header[32..34].copy_from_slice(&(u16::from(CHANNELS) * 2).to_le_bytes());
        header[34..36].copy_from_slice(&16_u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&data_len.to_le_bytes());
        header
    }

    struct WavWriter {
        file: BufWriter<File>,
        pcm: Vec<u8>,
        data_len: u32,
    }

    impl WavWriter {
        fn new(path: &Path) -> io::Result<Self> {
            if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = BufWriter::new(File::create(path)?);
            file.write_all(&wav_header(0))?;
            Ok(Self {
                file,
                pcm: Vec::new(),
                data_len: 0,
            })
        }

        fn push(&mut self, chunk: &AudioChunk) -> io::Result<()> {
            let bytes = u32::try_from(chunk.samples.len())
                .ok()
                .and_then(|samples| samples.checked_mul(2))
                .and_then(|bytes| self.data_len.checked_add(bytes))
                .filter(|total| *total <= u32::MAX - 36)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "WAV is too large"))?;
            self.pcm.clear();
            for &sample in &chunk.samples {
                let sample = sanitize(sample);
                let pcm = (sample * if sample < 0.0 { 32768.0 } else { 32767.0 }).round() as i16;
                self.pcm.extend_from_slice(&pcm.to_le_bytes());
            }
            self.file.write_all(&self.pcm)?;
            self.data_len = bytes;
            Ok(())
        }

        fn finish(mut self) -> io::Result<()> {
            self.file.seek(SeekFrom::Start(0))?;
            self.file.write_all(&wav_header(self.data_len))?;
            self.file.flush()
        }
    }

    struct EncodeProbe {
        framer: Framer,
        encoder: AudioEncoder,
        packets: u64,
        bytes: u64,
        packet: [u8; rs_audio::MAX_PACKET],
    }

    impl EncodeProbe {
        fn new() -> Result<Self, rs_audio::AudioError> {
            Ok(Self {
                framer: Framer::new(),
                encoder: AudioEncoder::new(&EncoderConfig::default())?,
                packets: 0,
                bytes: 0,
                packet: [0; rs_audio::MAX_PACKET],
            })
        }

        fn push(&mut self, chunk: &AudioChunk) -> Result<(), rs_audio::AudioError> {
            for frame in self
                .framer
                .push(chunk.capture_ts_us, chunk.channels, &chunk.samples)?
            {
                let bytes = self
                    .encoder
                    .encode(frame.pcm.as_slice(), &mut self.packet)?;
                self.packets += 1;
                self.bytes += bytes as u64;
            }
            Ok(())
        }

        fn report(&self, value: &mut Value) {
            value["packets"] = json!(self.packets);
            value["avg_packet_bytes"] = json!(if self.packets > 0 {
                self.bytes as f64 / self.packets as f64
            } else {
                0.0
            });
            let duration_s = self.packets as f64 * 0.01;
            value["kbps"] = json!(if duration_s > 0.0 {
                self.bytes as f64 * 8.0 / duration_s / 1000.0
            } else {
                0.0
            });
        }
    }

    async fn run(args: Args) -> Result<Value, ProbeError> {
        let mut encoding = if args.encode {
            Some(EncodeProbe::new()?)
        } else {
            None
        };
        let (tx, mut rx) = mpsc::channel(256);
        let started = Instant::now();
        let capture = MacAudioCapture::start(
            AudioCaptureConfig {
                sample_rate: SAMPLE_RATE,
                channels: CHANNELS,
                exclude_self: true,
            },
            tx,
        )?;
        let mut wav = WavWriter::new(&args.out)?;
        let mut measurements = Measurements::default();
        let mut beep = None;
        let mut child = None;
        let deadline = tokio::time::sleep(Duration::from_secs(args.seconds));
        let beep_timer = tokio::time::sleep(Duration::from_secs(1));
        tokio::pin!(deadline, beep_timer);
        loop {
            tokio::select! {
                biased;
                _ = &mut deadline => break,
                _ = &mut beep_timer, if args.beep && beep.is_none() => {
                    let mark = BeepMark { called: Instant::now(), unix_us: unix_micros() };
                    child = Some(Command::new("/usr/bin/afplay")
                        .arg("/System/Library/Sounds/Ping.aiff")
                        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                        .kill_on_drop(true).spawn()?);
                    beep = Some(mark);
                }
                maybe_chunk = rx.recv() => {
                    let chunk = maybe_chunk.ok_or_else(|| ProbeError::new("CaptureStopped", "audio sink closed unexpectedly"))?;
                    measurements.push(&chunk, Instant::now(), started, beep)?;
                    wav.push(&chunk)?;
                    if let Some(encoding) = &mut encoding { encoding.push(&chunk)?; }
                }
            }
        }
        let dropped = capture.finish();
        while let Some(chunk) = rx.recv().await {
            measurements.push(&chunk, Instant::now(), started, beep)?;
            wav.push(&chunk)?;
            if let Some(encoding) = &mut encoding {
                encoding.push(&chunk)?;
            }
        }
        wav.finish()?;
        if let Some(mut child) = child {
            let status = child.wait().await?;
            if !status.success() {
                return Err(ProbeError::new(
                    "BeepPlayback",
                    format!("afplay exited with {status}"),
                ));
            }
        }
        if measurements.chunks == 0 {
            return Err(ProbeError::new(
                "NoAudio",
                "ScreenCaptureKit produced no audio chunks",
            ));
        }
        let mut value = measurements.json(dropped);
        value["out"] = json!(args.out);
        if let Some(encoding) = encoding {
            encoding.report(&mut value);
        }
        if args.beep {
            value["beep_capture_latency_ms"] = json!(measurements.beep_capture_latency_ms);
            value["beep_called_ts_us"] = json!(beep.map(|mark| mark.unix_us));
            value["beep_sample_ts_us"] = json!(measurements.beep_sample_ts_us);
        }
        if !args.json {
            eprintln!("wrote {}", args.out.display());
        }
        Ok(value)
    }

    #[tokio::main(flavor = "current_thread")]
    pub async fn main() {
        let result = match parse_args() {
            Ok(args) => run(args).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => println!("{value}"),
            Err(error) => {
                println!(
                    "{}",
                    json!({ "error": { "code": error.code, "message": error.message } })
                );
                std::process::exit(1);
            }
        }
    }
}
