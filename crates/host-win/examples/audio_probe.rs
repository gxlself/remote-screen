//! Capture five seconds of default-render loopback audio for a Windows host
//! smoke test. `--out <file.f32>` writes little-endian interleaved stereo f32.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::{BufWriter, Error, ErrorKind, Write};
    use std::time::{Duration, Instant};

    use host_core::platform::AudioCaptureConfig;
    use host_win::audio::start_loopback;
    use tokio::sync::mpsc::error::TryRecvError;

    let mut output = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg != "--out" || output.is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "usage: audio_probe [--out file.f32]",
            )
            .into());
        }
        output = Some(
            args.next()
                .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "--out requires a path"))?,
        );
    }
    let mut file = output.map(File::create).transpose()?.map(BufWriter::new);
    let (sink, mut chunks) = tokio::sync::mpsc::channel(128);
    let capture = start_loopback(
        AudioCaptureConfig {
            sample_rate: 48_000,
            channels: 2,
            exclude_self: true,
        },
        sink,
    )?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut blocks = 0_u64;
    let mut total_frames = 0_u64;
    let mut peak = 0.0_f32;
    let mut first_ts = None;
    let mut last_ts = None;
    while Instant::now() < deadline {
        match chunks.try_recv() {
            Ok(chunk) => {
                blocks += 1;
                total_frames += u64::from(chunk.frames);
                first_ts.get_or_insert(chunk.capture_ts_us);
                last_ts = Some(chunk.capture_ts_us);
                for sample in &chunk.samples {
                    peak = peak.max(sample.abs());
                    if let Some(file) = file.as_mut() {
                        file.write_all(&sample.to_le_bytes())?;
                    }
                }
            }
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Disconnected) => {
                return Err(
                    Error::new(ErrorKind::BrokenPipe, "WASAPI capture worker stopped").into(),
                );
            }
        }
    }
    drop(capture);
    if let Some(file) = file.as_mut() {
        file.flush()?;
    }
    let timestamp_span = first_ts
        .zip(last_ts)
        .map(|(first, last)| last.saturating_sub(first))
        .unwrap_or(0);
    println!(
        "blocks={blocks} frames={total_frames} peak={peak:.6} first_last_delta_us={timestamp_span}"
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("audio_probe requires Windows WASAPI");
}
