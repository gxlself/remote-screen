#![cfg(feature = "desktop-it")]
#![cfg(target_os = "linux")]

use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions, TryLockError};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use host_core::platform::{AudioCaptureConfig, AudioChunk, PlatformError};
use host_linux::audio::{start_monitor_capture, AudioHandle, MAX_RATE, MIN_RATE};
use host_linux::pw::PwLib;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIG: AudioCaptureConfig = AudioCaptureConfig {
    sample_rate: 48_000,
    channels: 2,
    exclude_self: true,
};

fn assert_pipewire_layer() {
    assert_eq!(
        std::env::var("RS_IT_LAYER").as_deref(),
        Ok("pipewire"),
        "run under crates/host-linux/tests/desktop/run.sh pipewire"
    );
}

fn scratch_dir() -> PathBuf {
    std::env::var_os("TMPDIR")
        .expect("TMPDIR must point to the test scratch space")
        .into()
}

async fn lock_monitor() -> File {
    // nextest runs each test in a separate process. Serialize users of this
    // daemon's default monitor so another test's sine cannot spoil silence.
    let mut hasher = DefaultHasher::new();
    std::env::var_os("PIPEWIRE_RUNTIME_DIR")
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
        .expect("the desktop harness must supply a private runtime directory")
        .hash(&mut hasher);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(scratch_dir().join(format!("rs-v8t-audio-{:016x}.lock", hasher.finish())))
        .expect("open monitor test lock");
    timeout(TEST_TIMEOUT, async {
        loop {
            match file.try_lock() {
                Ok(()) => return,
                Err(TryLockError::WouldBlock) => sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("lock monitor: {error}"),
            }
        }
    })
    .await
    .expect("another audio test did not release the monitor");
    file
}

struct SineWav(PathBuf);

impl SineWav {
    fn new() -> Self {
        static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
        let path = scratch_dir().join(format!(
            "rs-v8t-audio-{}-{}.wav",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .expect("create sine WAV");
        let wav = Self(path);

        // One second of 48 kHz, stereo, PCM16 at amplitude 0.5.
        let data_len = CONFIG.sample_rate * u32::from(CONFIG.channels) * 2;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&u16::from(CONFIG.channels).to_le_bytes());
        bytes.extend_from_slice(&CONFIG.sample_rate.to_le_bytes());
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.extend_from_slice(&(u16::from(CONFIG.channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..CONFIG.sample_rate {
            let phase =
                std::f64::consts::TAU * 440.0 * f64::from(frame) / f64::from(CONFIG.sample_rate);
            let sample = (0.5 * phase.sin() * f64::from(i16::MAX)).round() as i16;
            for _ in 0..CONFIG.channels {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        file.write_all(&bytes).expect("write sine WAV");
        wav
    }
}

impl Drop for SineWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct Playback(Child);

impl Playback {
    fn start(wav: &SineWav) -> Self {
        let playback = Self(
            Command::new("pw-cat")
                .args(["--playback", "--target", "rs-null-sink"])
                .arg(&wav.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .expect("start pw-cat"),
        );
        let task = std::env::var("RS_TEST_TASK").unwrap_or_else(|_| "audio-integration".into());
        let mut pids = OpenOptions::new()
            .create(true)
            .append(true)
            .open(scratch_dir().join(format!("{task}.pids")))
            .expect("open test PID record");
        writeln!(pids, "{}", playback.0.id()).expect("record pw-cat PID");
        playback
    }

    async fn finish(&mut self) {
        timeout(TEST_TIMEOUT, async {
            loop {
                if let Some(status) = self.0.try_wait().expect("poll pw-cat") {
                    assert!(status.success(), "pw-cat failed: {status}");
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("pw-cat did not finish playback");
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // Child refers only to the PID started above; reap it even after a panic.
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn capture(capacity: usize) -> (AudioHandle, mpsc::Receiver<AudioChunk>) {
    let (sink, receiver) = mpsc::channel(capacity);
    let handle = start_monitor_capture(PwLib::load().expect("PipeWire library"), CONFIG, sink)
        .expect("start monitor capture");
    (handle, receiver)
}

fn chunk_rms(chunk: &AudioChunk) -> f64 {
    assert_eq!(chunk.channels, 2);
    assert_eq!(chunk.samples.len(), chunk.frames as usize * 2);
    assert!(chunk.samples.iter().all(|sample| sample.is_finite()));
    if chunk.samples.is_empty() {
        return 0.0;
    }
    (chunk
        .samples
        .iter()
        .map(|&sample| f64::from(sample).powi(2))
        .sum::<f64>()
        / chunk.samples.len() as f64)
        .sqrt()
}

async fn receive_sine(receiver: &mut mpsc::Receiver<AudioChunk>) {
    timeout(TEST_TIMEOUT, async {
        loop {
            let chunk = receiver.recv().await.expect("capture sink closed early");
            if chunk_rms(&chunk) > 0.1 {
                return;
            }
        }
    })
    .await
    .expect("no audible sine arrived from the monitor");
}

#[tokio::test]
async fn captures_sine() {
    assert_pipewire_layer();
    let _lock = lock_monitor().await;
    let wav = SineWav::new();
    let (handle, mut receiver) = capture(64);
    let mut playback = Playback::start(&wav);
    receive_sine(&mut receiver).await;
    playback.finish().await;
    assert_eq!(handle.stats().bad_buffers, 0);
}

#[tokio::test]
async fn silence_is_quiet() {
    assert_pipewire_layer();
    let _lock = lock_monitor().await;
    let (handle, mut receiver) = capture(64);
    timeout(TEST_TIMEOUT, async {
        // Observe a full second, including the case where no buffers arrive.
        let observation = sleep(Duration::from_secs(1));
        tokio::pin!(observation);
        loop {
            tokio::select! {
                _ = &mut observation => return,
                chunk = receiver.recv() => {
                    let chunk = chunk.expect("capture sink closed early");
                    assert!(chunk_rms(&chunk) < 0.01, "monitor produced non-silent audio");
                }
            }
        }
    })
    .await
    .expect("silence observation hung");
    assert_eq!(handle.stats().bad_buffers, 0);
}

#[tokio::test]
async fn bad_params() {
    assert_pipewire_layer();
    let lib = PwLib::load().expect("PipeWire library");
    for (sample_rate, channels) in [
        (0, 2),
        (MIN_RATE - 1, 2),
        (MAX_RATE + 1, 2),
        (48_000, 0),
        (48_000, 9),
    ] {
        let (sink, mut receiver) = mpsc::channel(1);
        let result = start_monitor_capture(
            lib.clone(),
            AudioCaptureConfig {
                sample_rate,
                channels,
                ..CONFIG
            },
            sink,
        );
        assert!(matches!(result, Err(PlatformError::Unsupported)));
        assert!(timeout(TEST_TIMEOUT, receiver.recv())
            .await
            .expect("rejected capture retained its sink")
            .is_none());
    }
}

#[tokio::test]
async fn drop_stops() {
    assert_pipewire_layer();
    let _lock = lock_monitor().await;
    let wav = SineWav::new();
    let (handle, mut receiver) = capture(64);
    let mut playback = Playback::start(&wav);
    receive_sine(&mut receiver).await;
    let started = Instant::now();
    drop(handle);
    assert!(started.elapsed() < Duration::from_millis(100));
    timeout(TEST_TIMEOUT, async {
        while receiver.recv().await.is_some() {}
    })
    .await
    .expect("dropping the handle must release the sink sender");
    playback.finish().await;
}

#[tokio::test]
async fn full_sink_drops() {
    assert_pipewire_layer();
    let _lock = lock_monitor().await;
    let wav = SineWav::new();
    let (handle, _receiver) = capture(1);
    let mut playback = Playback::start(&wav);
    timeout(TEST_TIMEOUT, async {
        let mut previous = 0;
        let mut advances = 0;
        loop {
            let stats = handle.stats();
            let total = stats.chunks + stats.dropped;
            if stats.dropped > 0 && total > previous {
                advances += 1;
                previous = total;
                if advances == 3 {
                    assert_eq!(stats.chunks, 1);
                    return;
                }
            }
            assert!(
                playback.0.try_wait().expect("poll pw-cat").is_none(),
                "playback ended before the full sink kept dropping buffers"
            );
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the process callback blocked on a full sink");
    playback.finish().await;
    assert_eq!(handle.stats().bad_buffers, 0);
}
