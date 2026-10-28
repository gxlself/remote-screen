mod support;

use std::{ptr, sync::Arc, time::Duration};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_audio::metrics::rms_dbfs;
use rs_mobile::rscore::{
    api_sessions::{self as sessions, sessions_set_audio, sessions_set_quality},
    audio::AudioTable,
    dto_core::{GoodbyeReason, OpenIntent, OpenOpts, SessionPhase},
    ffi_audio::{rs_audio_close, rs_audio_open, rs_audio_render, RsAudio},
    runtime::{install_core, take_core, Core},
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_core::trust::Permission;
use tokio::{
    sync::{Mutex, MutexGuard},
    time::{sleep, timeout, Instant},
};

static GLOBAL_CORE: Mutex<()> = Mutex::const_new(());
const TWO_SECONDS: Duration = Duration::from_secs(2);

fn opts(audio: Option<bool>) -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

struct Fixture {
    core: Arc<Core>,
    host: TestHost,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let serial = GLOBAL_CORE.lock().await;
        let core = support::test_core();
        support::with_identity(&core, "Audio test device");
        install_core(core.clone());
        let host = TestHost::start_with_platform(
            FakePlatform::new(),
            TestHostOptions {
                tcp: false,
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host.host_id.clone(),
                fp: host.fp,
                name: "Audio test host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        Self {
            core,
            host,
            _serial: serial,
        }
    }

    async fn open(&self, audio: Option<bool>) -> (u64, Puller) {
        let id = sessions::open(&self.core, &self.host.host_id, opts(audio)).unwrap();
        timeout(TWO_SECONDS, async {
            loop {
                if self
                    .core
                    .state::<SessionTable>()
                    .get(id)
                    .is_some_and(|entry| entry.summary().phase == SessionPhase::Streaming)
                {
                    let handle = rs_audio_open(id);
                    assert!(!handle.is_null());
                    return (id, Puller(handle));
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("audio session streaming within 2 s")
    }

    async fn close(&self, id: u64, puller: &Puller) {
        sessions::close(&self.core, id);
        assert!(self.core.state::<AudioTable>().get(id).is_none());
        assert!(rs_audio_open(id).is_null());
        assert!(puller.render().iter().all(|&x| x == 0.0));
        timeout(TWO_SECONDS, async {
            while self.core.state::<SessionTable>().get(id).is_some() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("closed session removed");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        take_core();
        self.core.shutdown();
    }
}

struct Puller(*mut RsAudio);

impl Puller {
    fn render(&self) -> [f32; 480] {
        let mut left = [1.0; 480];
        let mut right = [1.0; 480];
        // SAFETY: The handle remains open until Drop; arrays are disjoint and
        // contain exactly 480 frames with no other access during the call.
        assert_eq!(
            unsafe { rs_audio_render(self.0, left.as_mut_ptr(), right.as_mut_ptr(), 480) },
            480
        );
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
        left
    }

    async fn audible(&self, limit: Duration) {
        timeout(limit, async {
            loop {
                if rms_dbfs(&self.render()) > -30.0 {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("non-silent audio before deadline");
    }

    async fn silent_for(&self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            assert!(self.render().iter().all(|&x| x == 0.0));
            sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for Puller {
    fn drop(&mut self) {
        // SAFETY: Every render completed synchronously and this is the only close.
        unsafe { rs_audio_close(self.0) };
        self.0 = ptr::null_mut();
    }
}

#[tokio::test]
async fn requested_audio_is_audible_and_closed_handles_are_silent() {
    let fixture = Fixture::new().await;
    let (id, puller) = fixture.open(Some(true)).await;
    puller.audible(TWO_SECONDS).await;
    assert!(
        fixture
            .core
            .state::<AudioTable>()
            .get(id)
            .unwrap()
            .stats()
            .jitter
            .received
            > 0
    );
    fixture.close(id, &puller).await;
}

#[tokio::test]
async fn default_audio_stays_silent_then_switches_on_and_off() {
    let fixture = Fixture::new().await;
    let (id, puller) = fixture.open(None).await;
    puller.silent_for(TWO_SECONDS).await;
    assert_eq!(
        fixture
            .core
            .state::<AudioTable>()
            .get(id)
            .unwrap()
            .stats()
            .jitter
            .received,
        0
    );
    sessions_set_audio(id, true).unwrap();
    puller.audible(Duration::from_secs(1)).await;
    sessions_set_audio(id, false).unwrap();
    let ramp = puller.render();
    assert!(ramp[239..].iter().all(|&x| x == 0.0));
    puller.silent_for(Duration::from_millis(300)).await;
    // Re-enabling preserves the sink and recovers after the muted interval.
    sessions_set_audio(id, true).unwrap();
    puller.audible(Duration::from_secs(1)).await;
    fixture.close(id, &puller).await;
}

#[tokio::test]
async fn unknown_session_errors_match_quality_and_failed_opens_remove_audio() {
    let fixture = Fixture::new().await;
    assert_eq!(
        sessions_set_audio(999, true).unwrap_err(),
        sessions_set_quality(999, "balanced".into()).unwrap_err(),
    );
    assert!(fixture.core.state::<AudioTable>().get(999).is_none());
    // Exercise connecting-close and failed-open cleanup independently of wire
    // timing, using the same SessionEntry and parameter builder as run_session.
    let entry = fixture.core.state::<SessionTable>().insert(
        &fixture.host.host_id,
        "Audio test host",
        opts(Some(true)),
        fixture.core.now_us(),
    );
    let params = sessions::session_params(&fixture.core, &entry).unwrap();
    assert!(params.audio);
    assert!(params.audio_sink.is_some());
    let pointer = rs_audio_open(entry.id);
    assert!(!pointer.is_null());
    let puller = Puller(pointer);
    sessions::close(&fixture.core, entry.id);
    sessions_set_audio(entry.id, true).unwrap();
    assert!(fixture.core.state::<AudioTable>().get(entry.id).is_none());
    assert!(puller.render().iter().all(|&x| x == 0.0));
    sessions::finish_open(
        &fixture.core,
        &entry,
        None,
        Some(GoodbyeReason::UserClosed),
        "cancelled".into(),
    );
    assert!(fixture.core.state::<SessionTable>().get(entry.id).is_none());
    assert!(rs_audio_open(entry.id).is_null());
}
