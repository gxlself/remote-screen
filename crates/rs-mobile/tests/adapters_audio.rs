//! Adapter session audio through the rs_audio_* ABI.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use rs_mobile::rscore::adapters::{
    audio::AdapterAudioTable,
    driver::{
        AdapterError, Dial, DriverHost, DriverInput, DriverParams, FrameSink, ProtocolDriver,
    },
    dto::{AdapterOpenOpts, AdapterProtocol, AdapterTarget},
    session::{self, open_with},
    ADAPTER_ID_BASE,
};
use rs_mobile::rscore::{
    audio::AudioTable,
    ffi_audio::{
        rs_audio_close, rs_audio_open, rs_audio_render_interleaved, rs_audio_set_muted,
        rs_audio_stats, RsAudio, RsAudioStats,
    },
    runtime::{install_core, take_core, Core},
};
use rsp_transport::net::NetStream;
use tokio::sync::mpsc as tokio_mpsc;

static GLOBAL_CORE: Mutex<()> = Mutex::new(());
const FIVE_SECONDS: Duration = Duration::from_secs(5);

type ScriptFuture = Pin<Box<dyn Future<Output = Result<(), AdapterError>> + Send + 'static>>;
type Script =
    dyn Fn(Arc<dyn DriverHost>, tokio_mpsc::Receiver<DriverInput>) -> ScriptFuture + Send + Sync;

struct ScriptDriver(Arc<Script>);

#[async_trait]
impl ProtocolDriver for ScriptDriver {
    fn protocol(&self) -> AdapterProtocol {
        AdapterProtocol::Rdp
    }

    async fn run(
        &self,
        _params: DriverParams,
        host: Arc<dyn DriverHost>,
        _sink: Arc<dyn FrameSink>,
        input: tokio_mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError> {
        (self.0)(host, input).await
    }
}

struct NoDial;

#[async_trait]
impl Dial for NoDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        Err(AdapterError::Refused)
    }
}

struct Installed {
    core: Arc<Core>,
    _serial: MutexGuard<'static, ()>,
}

impl Installed {
    fn new() -> Self {
        let serial = GLOBAL_CORE.lock().unwrap_or_else(|e| e.into_inner());
        let core = support::test_core();
        install_core(core.clone());
        Self {
            core,
            _serial: serial,
        }
    }

    /// Open an RDP session whose driver pushes `pcm` (48 kHz stereo) through
    /// DriverHost::audio, reports it, then waits for the session to close.
    fn open_playing(&self, pcm: Vec<i16>) -> u64 {
        let (pushed, pushed_rx) = mpsc::channel();
        let pcm = Arc::new(pcm);
        let script: Arc<Script> = Arc::new(
            move |host: Arc<dyn DriverHost>,
                  mut input: tokio_mpsc::Receiver<DriverInput>|
                  -> ScriptFuture {
                let pushed = pushed.clone();
                let pcm = pcm.clone();
                Box::pin(async move {
                    let audio = host.audio().expect("adapter sessions have audio");
                    audio.format(48_000, 2);
                    audio.samples(&pcm);
                    pushed.send(()).unwrap();
                    let _ = input.recv().await;
                    Ok(())
                })
            },
        );
        let target = AdapterTarget {
            protocol: AdapterProtocol::Rdp,
            host_id: None,
            addr: "192.0.2.10".to_owned(),
            port: None,
        };
        let id = self
            .core
            .block_on(open_with(
                &self.core,
                target,
                AdapterOpenOpts::default(),
                Arc::new(ScriptDriver(script)),
                Arc::new(NoDial),
            ))
            .expect("open adapter session");
        pushed_rx
            .recv_timeout(FIVE_SECONDS)
            .expect("driver pushed audio");
        id
    }

    fn close(&self, id: u64) {
        assert!(session::close(&self.core, id));
        let started = std::time::Instant::now();
        while self.core.state::<AdapterAudioTable>().get(id).is_some() {
            assert!(started.elapsed() < FIVE_SECONDS, "session {id} did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        take_core();
        self.core.shutdown();
    }
}

struct Handle(*mut RsAudio);

impl Handle {
    fn open(id: u64) -> Self {
        let pointer = rs_audio_open(id);
        assert!(!pointer.is_null(), "rs_audio_open({id})");
        Self(pointer)
    }

    fn pull(&self, frames: usize) -> Vec<f32> {
        let mut lr = vec![1.0; frames * 2];
        // SAFETY: The handle is open and lr holds exactly this many frames.
        let rendered =
            unsafe { rs_audio_render_interleaved(self.0, lr.as_mut_ptr(), frames as u32) };
        assert_eq!(rendered, frames as u32);
        lr
    }

    fn mute(&self, muted: bool) {
        // SAFETY: This handle remains open until Drop.
        unsafe { rs_audio_set_muted(self.0, muted) };
    }

    fn stats(&self) -> RsAudioStats {
        let mut stats = RsAudioStats::default();
        // SAFETY: Both the handle and output storage are live and aligned.
        unsafe { rs_audio_stats(self.0, &mut stats) };
        stats
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: Render calls are synchronous; this is the only close.
        unsafe { rs_audio_close(self.0) };
    }
}

/// 48 kHz stereo: a 440 Hz tone on the left, half of it on the right.
fn tone(frames: usize) -> Vec<i16> {
    (0..frames)
        .flat_map(|index| {
            let phase = std::f64::consts::TAU * 440.0 * index as f64 / 48_000.0;
            let left = (phase.sin() * 16_000.0) as i16;
            [left, left / 2]
        })
        .collect()
}

fn scaled(pcm: &[i16]) -> Vec<f32> {
    pcm.iter()
        .map(|&sample| f32::from(sample) / 32768.0)
        .collect()
}

#[test]
fn driver_audio_reaches_the_native_player_until_close() {
    let installed = Installed::new();
    let pcm = tone(9_600); // 200 ms
    let id = installed.open_playing(pcm.clone());
    assert!(id >= ADAPTER_ID_BASE);
    let handle = Handle::open(id);

    let played = handle.pull(2_400);
    assert!(played.iter().any(|&x| x != 0.0));
    assert_eq!(played, scaled(&pcm[..4_800]));
    let stats = handle.stats();
    assert!(stats.playing);
    assert_eq!(stats.packets, 1);
    assert_eq!(stats.target_ms, 80);
    assert_eq!(stats.level_ms, 150);
    assert_eq!(stats.contended, 0);

    // 150 ms are still buffered; once the session ends the handle is silent.
    installed.close(id);
    assert!(handle.pull(2_400).iter().all(|&x| x == 0.0));
    assert!(!handle.stats().playing);
    assert!(rs_audio_open(id).is_null());
}

#[test]
fn mute_through_an_adapter_handle() {
    let installed = Installed::new();
    let id = installed.open_playing(tone(9_600));
    let handle = Handle::open(id);
    assert!(handle.pull(480).iter().any(|&x| x != 0.0));
    handle.mute(true);
    let faded = handle.pull(480);
    assert!(faded[..480].iter().any(|&x| x != 0.0));
    assert!(faded[480..].iter().all(|&x| x == 0.0));
    assert!(!handle.stats().playing);
    handle.mute(false);
    let restored = handle.pull(480);
    assert!(restored[480..].iter().any(|&x| x != 0.0));
    installed.close(id);
}

#[test]
fn unknown_adapter_ids_match_unknown_rsp_ids() {
    let installed = Installed::new();
    assert!(rs_audio_open(999).is_null());
    assert!(rs_audio_open(ADAPTER_ID_BASE).is_null());
    assert!(rs_audio_open(ADAPTER_ID_BASE + 999).is_null());
    assert!(rs_audio_open(u64::MAX).is_null());
    // Adapter ids never fall through to the RSP table, nor RSP ids to the
    // adapter table.
    installed
        .core
        .state::<AudioTable>()
        .get_or_create(ADAPTER_ID_BASE + 5)
        .unwrap();
    assert!(rs_audio_open(ADAPTER_ID_BASE + 5).is_null());
    installed.core.state::<AdapterAudioTable>().get_or_create(5);
    assert!(rs_audio_open(5).is_null());
    installed
        .core
        .state::<AudioTable>()
        .get_or_create(5)
        .unwrap();
    drop(Handle::open(5));

    let id = installed.open_playing(tone(4_800));
    drop(Handle::open(id));
    installed.close(id);
    assert!(rs_audio_open(id).is_null());
    // NULL handles are ignored exactly as for RSP sessions.
    // SAFETY: NULL is an accepted argument for every rs_audio_* call.
    unsafe {
        assert_eq!(
            rs_audio_render_interleaved(ptr::null_mut(), ptr::null_mut(), 480),
            0
        );
        rs_audio_close(ptr::null_mut());
    }
}
