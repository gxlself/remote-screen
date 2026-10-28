#![cfg(feature = "testkit")]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_core::{decode, encode_vec, impl_as_any};
use ironrdp_dvc::pdu::{
    CapabilitiesRequestPdu, CapsVersion, ClosePdu, CreateRequestPdu, CreationStatus, DataPdu,
    DrdynvcClientPdu, DrdynvcDataPdu, DrdynvcServerPdu,
};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_pdu::{gcc::ChannelName, PduResult};
use ironrdp_rdpsnd::client::{DVC_CHANNEL_NAME, LOSSY_DVC_CHANNEL_NAME};
use ironrdp_rdpsnd::pdu::{
    AudioFormat, ClientAudioOutputPdu, PitchPdu, ServerAudioFormatPdu, ServerAudioOutputPdu,
    TrainingPdu, Version, VolumePdu, Wave2Pdu, WaveFormat,
};
use ironrdp_rdpsnd::server::{NegotiatedFormat, RdpsndError, RdpsndServer, RdpsndServerHandler};
use ironrdp_server::{Acceptor, StaticChannelFactory};
use ironrdp_svc::{CompressionCondition, SvcMessage, SvcProcessor, SvcServerProcessor};
use rs_rdp::client::{connect, RdpConnection};
use rs_rdp::ext::ExtCtx;
use rs_rdp::testkit::{FakeRdpServer, FakeSecurity};
use rs_rdp::{CertCheck, RdpAudioSink, RdpConfig, RdpSink, Rect};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("audio test exceeded its 20-second wall-clock budget");
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AudioCall {
    Format(u32, u16),
    Samples(Vec<i16>),
    Stop,
}

#[derive(Default)]
struct RecAudio {
    calls: Mutex<Vec<AudioCall>>,
    changed: Notify,
}

impl RecAudio {
    fn record(&self, call: AudioCall) {
        self.calls.lock().unwrap().push(call);
        self.changed.notify_one();
    }

    async fn until(&self, predicate: impl Fn(&[AudioCall]) -> bool) -> Vec<AudioCall> {
        loop {
            let notified = self.changed.notified();
            {
                let calls = self.calls.lock().unwrap();
                if predicate(&calls) {
                    return calls.clone();
                }
            }
            notified.await;
        }
    }
}

impl RdpAudioSink for RecAudio {
    fn format(&self, rate: u32, channels: u16) {
        self.record(AudioCall::Format(rate, channels));
    }

    fn samples(&self, pcm: &[i16]) {
        self.record(AudioCall::Samples(pcm.to_vec()));
    }

    fn stop(&self) {
        self.record(AudioCall::Stop);
    }
}

#[derive(Default)]
struct Frames(Notify);

impl RdpSink for Frames {
    fn resize(&self, _: u16, _: u16) {}
    fn update(&self, _: &[u8], _: usize, _: Rect) {}
    fn commit(&self) {
        self.0.notify_one();
    }
    fn pointer_bitmap(&self, _: u16, _: u16, _: u16, _: u16, _: &[u8]) {}
    fn pointer_hidden(&self) {}
    fn pointer_default(&self) {}
    fn pointer_pos(&self, _: u16, _: u16) {}
}

struct AcceptCertificate;

#[async_trait]
impl CertCheck for AcceptCertificate {
    async fn check(&self, _: [u8; 32]) -> bool {
        true
    }
}

fn config(audio: Option<Arc<dyn RdpAudioSink>>) -> RdpConfig {
    RdpConfig {
        server_name: "127.0.0.1".into(),
        credentials: None,
        width: 800,
        height: 600,
        scale_percent: 100,
        client_name: "rdp-audio-test".into(),
        cert: Arc::new(AcceptCertificate),
        gfx: false,
        monitors: 1,
        audio,
        clipboard: false,
    }
}

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: rate,
        n_avg_bytes_per_sec: rate * 4,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    }
}

#[derive(Debug, Default)]
struct ServerRecord {
    channel_started: bool,
    client_formats: Option<Vec<AudioFormat>>,
}

#[derive(Debug)]
struct SoundHandler {
    formats: Vec<AudioFormat>,
    rate: u32,
}

impl RdpsndServerHandler for SoundHandler {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn choose_format<'a>(
        &mut self,
        common: &'a [NegotiatedFormat],
    ) -> Option<&'a NegotiatedFormat> {
        common
            .iter()
            .find(|format| format.format().n_samples_per_sec == self.rate)
    }

    fn start(&mut self, _: &NegotiatedFormat) -> Result<(), Box<dyn RdpsndError>> {
        Ok(())
    }

    fn stop(&mut self) {}
}

// Wrap the real server processor to observe the actual client format PDU, not
// merely the common formats handed to choose_format (which hide extra entries).
struct SoundChannel {
    inner: RdpsndServer,
    record: Arc<Mutex<ServerRecord>>,
    waves: Option<Vec<Vec<u8>>>,
    close: bool,
}

impl std::fmt::Debug for SoundChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundChannel")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl_as_any!(SoundChannel);

impl SvcProcessor for SoundChannel {
    fn channel_name(&self) -> ChannelName {
        self.inner.channel_name()
    }

    fn compression_condition(&self) -> CompressionCondition {
        self.inner.compression_condition()
    }

    fn start(&mut self) -> PduResult<Vec<SvcMessage>> {
        self.record.lock().unwrap().channel_started = true;
        self.inner.start()
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        if let ClientAudioOutputPdu::AudioFormat(formats) = decode(payload).unwrap() {
            self.record.lock().unwrap().client_formats = Some(formats.formats);
        }
        let mut messages = self.inner.process(payload)?;
        if self.inner.is_ready() {
            if let Some(waves) = self.waves.take() {
                for wave in waves {
                    let next: Vec<SvcMessage> = self.inner.wave(wave, 0)?.into();
                    messages.extend(next);
                }
                if self.close {
                    let close: Vec<SvcMessage> = self.inner.close()?.into();
                    messages.extend(close);
                }
            }
        }
        Ok(messages)
    }
}

impl SvcServerProcessor for SoundChannel {}

struct SoundFactory {
    formats: Vec<AudioFormat>,
    rate: u32,
    waves: Vec<Vec<u8>>,
    close: bool,
    record: Arc<Mutex<ServerRecord>>,
}

impl StaticChannelFactory for SoundFactory {
    fn attach(&self, acceptor: &mut Acceptor) {
        acceptor.attach_static_channel(SoundChannel {
            inner: RdpsndServer::new(Box::new(SoundHandler {
                formats: self.formats.clone(),
                rate: self.rate,
            })),
            record: Arc::clone(&self.record),
            waves: Some(self.waves.clone()),
            close: self.close,
        });
    }
}

async fn server(
    formats: Vec<AudioFormat>,
    rate: u32,
    waves: &[Vec<i16>],
    close: bool,
) -> (FakeRdpServer, Arc<Mutex<ServerRecord>>) {
    let record = Arc::new(Mutex::new(ServerRecord::default()));
    let factory = SoundFactory {
        formats,
        rate,
        waves: waves
            .iter()
            .map(|wave| {
                wave.iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect()
            })
            .collect(),
        close,
        record: Arc::clone(&record),
    };
    let server = FakeRdpServer::start_with(FakeSecurity::Tls, 800, 600, move |builder| {
        builder.with_static_channel_factory(Box::new(factory))
    })
    .await
    .unwrap();
    (server, record)
}

async fn open(
    server: &FakeRdpServer,
    audio: Option<Arc<dyn RdpAudioSink>>,
    frames: Arc<Frames>,
) -> RdpConnection {
    let stream = TcpStream::connect(server.addr()).await.unwrap();
    stream.set_nodelay(true).unwrap();
    connect(stream, config(audio), frames).await.unwrap()
}

async fn session(connection: RdpConnection, check: impl Future<Output = ()>) {
    let (tx, rx) = mpsc::channel(1);
    let (result, ()) = tokio::join!(connection.run(rx), async {
        check.await;
        drop(tx);
    });
    assert_eq!(result, Ok(()));
}

async fn one_wave(audio: &RecAudio) -> Vec<AudioCall> {
    audio
        .until(|calls| {
            calls
                .iter()
                .any(|call| matches!(call, AudioCall::Samples(_)))
        })
        .await
}

fn playing(calls: &[AudioCall]) -> &[AudioCall] {
    let first = calls
        .iter()
        .position(|call| matches!(call, AudioCall::Format(..)))
        .expect("format must be reported before samples");
    &calls[first..]
}

#[tokio::test]
async fn no_audio_no_channel() {
    wall_clock(async {
        let (server, record) = server(vec![pcm(48_000)], 48_000, &[], false).await;
        let frames = Arc::new(Frames::default());
        let connection = open(&server, None, Arc::clone(&frames)).await;
        // A real graphics update proves the server has reached its active loop;
        // channel absence is asserted without a race or a timing-based sleep.
        server.push_bitmap(0, 0, 16, 16, vec![0; 16 * 16 * 4]);
        session(connection, async {
            frames.0.notified().await;
            let record = record.lock().unwrap();
            assert!(!record.channel_started);
            assert_eq!(record.client_formats, None);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn pcm_48k() {
    wall_clock(async {
        let samples: Vec<i16> = (-480..480).collect();
        let (server, _) = server(
            vec![pcm(44_100), pcm(48_000)],
            48_000,
            std::slice::from_ref(&samples),
            false,
        )
        .await;
        let audio = Arc::new(RecAudio::default());
        let connection = open(&server, Some(audio.clone()), Arc::new(Frames::default())).await;
        session(connection, async {
            let calls = one_wave(&audio).await;
            assert_eq!(
                playing(&calls),
                &[AudioCall::Format(48_000, 2), AudioCall::Samples(samples)]
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn pcm_44k() {
    wall_clock(async {
        let samples = vec![i16::MIN, -1, 0, i16::MAX];
        let (server, record) = server(
            vec![pcm(44_100)],
            44_100,
            std::slice::from_ref(&samples),
            false,
        )
        .await;
        let audio = Arc::new(RecAudio::default());
        let connection = open(&server, Some(audio.clone()), Arc::new(Frames::default())).await;
        session(connection, async {
            let calls = one_wave(&audio).await;
            assert_eq!(
                playing(&calls),
                &[AudioCall::Format(44_100, 2), AudioCall::Samples(samples)]
            );
            assert_eq!(
                record.lock().unwrap().client_formats,
                Some(vec![pcm(44_100)])
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn close_calls_stop() {
    wall_clock(async {
        let samples = vec![1, -1];
        let (server, _) = server(
            vec![pcm(48_000)],
            48_000,
            std::slice::from_ref(&samples),
            true,
        )
        .await;
        let audio = Arc::new(RecAudio::default());
        let connection = open(&server, Some(audio.clone()), Arc::new(Frames::default())).await;
        session(connection, async {
            // Initial negotiation also resets the upstream handler. Only a stop
            // following the actual wave proves that the Close PDU was handled.
            let calls = audio
                .until(|calls| {
                    calls
                        .windows(2)
                        .any(|pair| matches!(pair, [AudioCall::Samples(_), AudioCall::Stop]))
                })
                .await;
            assert_eq!(
                playing(&calls),
                &[
                    AudioCall::Format(48_000, 2),
                    AudioCall::Samples(samples),
                    AudioCall::Stop,
                ]
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn formats_advertised() {
    wall_clock(async {
        let mut mono = pcm(48_000);
        mono.n_channels = 1;
        mono.n_block_align = 2;
        mono.n_avg_bytes_per_sec = 96_000;
        let (server, record) = server(
            vec![pcm(44_100), pcm(96_000), mono, pcm(48_000)],
            48_000,
            &[vec![0, 0]],
            false,
        )
        .await;
        let audio = Arc::new(RecAudio::default());
        let connection = open(&server, Some(audio.clone()), Arc::new(Frames::default())).await;
        session(connection, async {
            one_wave(&audio).await;
            assert_eq!(
                record.lock().unwrap().client_formats,
                Some(vec![pcm(48_000), pcm(44_100)])
            );
        })
        .await;
    })
    .await;
}

fn exchange(client: &mut DrdynvcClient, pdu: DrdynvcServerPdu) -> Vec<DrdynvcClientPdu> {
    client
        .process(&encode_vec(&pdu).unwrap())
        .unwrap()
        .iter()
        .map(|message| decode(&message.encode_unframed_pdu().unwrap()).unwrap())
        .collect()
}

fn dynamic_client(audio: Option<Arc<dyn RdpAudioSink>>) -> DrdynvcClient {
    let cfg = config(audio.clone());
    let ctx = ExtCtx {
        sink: Arc::new(Frames::default()),
        audio,
        wake: Arc::new(Notify::new()),
        background: Default::default(),
    };
    let mut client = rs_rdp::audio::dynamic_channels(&cfg, &ctx, DrdynvcClient::new());
    let replies = exchange(
        &mut client,
        DrdynvcServerPdu::Capabilities(CapabilitiesRequestPdu::new(CapsVersion::V1, None)),
    );
    assert!(matches!(
        replies.as_slice(),
        [DrdynvcClientPdu::Capabilities(_)]
    ));
    client
}

fn create_dvc(client: &mut DrdynvcClient, id: u32, name: &str, status: CreationStatus) {
    let replies = exchange(
        client,
        DrdynvcServerPdu::Create(CreateRequestPdu::new(id, name.into())),
    );
    let [DrdynvcClientPdu::Create(reply)] = replies.as_slice() else {
        panic!("expected a DVC create response");
    };
    assert_eq!(reply.channel_id(), id);
    assert_eq!(reply.creation_status(), status);
}

fn dvc_audio(
    client: &mut DrdynvcClient,
    id: u32,
    pdu: ServerAudioOutputPdu<'_>,
) -> Vec<ClientAudioOutputPdu> {
    exchange(
        client,
        DrdynvcServerPdu::Data(DrdynvcDataPdu::Data(DataPdu::new(
            id,
            encode_vec(&pdu).unwrap(),
        ))),
    )
    .into_iter()
    .map(|reply| {
        let DrdynvcClientPdu::Data(DrdynvcDataPdu::Data(data)) = reply else {
            panic!("expected an unfragmented rdpsnd response");
        };
        decode(data.data()).unwrap()
    })
    .collect()
}

fn negotiate_dvc(
    client: &mut DrdynvcClient,
    id: u32,
    formats: Vec<AudioFormat>,
) -> Vec<AudioFormat> {
    let replies = dvc_audio(
        client,
        id,
        ServerAudioOutputPdu::AudioFormat(ServerAudioFormatPdu {
            version: Version::V8,
            formats,
        }),
    );
    let [ClientAudioOutputPdu::AudioFormat(formats), ClientAudioOutputPdu::QualityMode(_)] =
        replies.as_slice()
    else {
        panic!("expected the client formats and quality mode");
    };
    let training = dvc_audio(
        client,
        id,
        ServerAudioOutputPdu::Training(TrainingPdu {
            timestamp: 42,
            data: Vec::new(),
        }),
    );
    assert!(matches!(
        training.as_slice(),
        [ClientAudioOutputPdu::TrainingConfirm(_)]
    ));
    formats.formats.clone()
}

fn wave(format_no: u16, samples: &[i16]) -> ServerAudioOutputPdu<'static> {
    ServerAudioOutputPdu::Wave2(Wave2Pdu {
        timestamp: 0,
        format_no,
        block_no: 0,
        audio_timestamp: 0,
        data: samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>()
            .into(),
    })
}

#[tokio::test]
async fn no_audio_no_dynamic_channels() {
    wall_clock(async {
        let mut client = dynamic_client(None);
        create_dvc(
            &mut client,
            1,
            DVC_CHANNEL_NAME,
            CreationStatus::NO_LISTENER,
        );
        create_dvc(
            &mut client,
            2,
            LOSSY_DVC_CHANNEL_NAME,
            CreationStatus::NO_LISTENER,
        );
    })
    .await;
}

#[tokio::test]
async fn dynamic_channels_share_formats_and_reopen() {
    wall_clock(async {
        let audio = Arc::new(RecAudio::default());
        let mut client = dynamic_client(Some(audio.clone()));
        create_dvc(&mut client, 1, DVC_CHANNEL_NAME, CreationStatus::OK);
        create_dvc(&mut client, 2, LOSSY_DVC_CHANNEL_NAME, CreationStatus::OK);
        assert_eq!(
            negotiate_dvc(&mut client, 1, vec![pcm(44_100), pcm(48_000)]),
            vec![pcm(48_000), pcm(44_100)]
        );
        // Only the reliable channel negotiated. The lossy channel must resolve
        // index 1 against that client list and must not send a Wave Confirm.
        assert!(dvc_audio(&mut client, 2, wave(1, &[i16::MIN, i16::MAX])).is_empty());
        assert_eq!(
            playing(&audio.calls.lock().unwrap()),
            &[
                AudioCall::Format(44_100, 2),
                AudioCall::Samples(vec![i16::MIN, i16::MAX]),
            ]
        );
        for id in [2, 1] {
            exchange(&mut client, DrdynvcServerPdu::Close(ClosePdu::new(id)));
        }
        assert_eq!(audio.calls.lock().unwrap().last(), Some(&AudioCall::Stop));
        audio.calls.lock().unwrap().clear();

        // Factories remain available after Close, including under new IDs.
        create_dvc(&mut client, 3, DVC_CHANNEL_NAME, CreationStatus::OK);
        create_dvc(&mut client, 4, LOSSY_DVC_CHANNEL_NAME, CreationStatus::OK);
        assert!(dvc_audio(&mut client, 4, wave(1, &[5, 6])).is_empty());
        assert!(audio
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| *call == AudioCall::Stop));
        assert_eq!(
            negotiate_dvc(&mut client, 3, vec![pcm(48_000)]),
            vec![pcm(48_000)]
        );
        dvc_audio(&mut client, 4, wave(0, &[7, 8]));
        assert_eq!(
            playing(&audio.calls.lock().unwrap()),
            &[AudioCall::Format(48_000, 2), AudioCall::Samples(vec![7, 8])]
        );
    })
    .await;
}

#[tokio::test]
async fn wave_blocks_format_changes_and_controls() {
    wall_clock(async {
        let audio = Arc::new(RecAudio::default());
        let mut client = dynamic_client(Some(audio.clone()));
        create_dvc(&mut client, 1, DVC_CHANNEL_NAME, CreationStatus::OK);
        negotiate_dvc(&mut client, 1, vec![pcm(48_000), pcm(44_100)]);
        audio.calls.lock().unwrap().clear();
        let confirm = dvc_audio(&mut client, 1, wave(0, &[1, -2]));
        assert!(matches!(
            confirm.as_slice(),
            [ClientAudioOutputPdu::WaveConfirm(_)]
        ));
        dvc_audio(&mut client, 1, wave(0, &[3, -4, 5, -6]));
        dvc_audio(
            &mut client,
            1,
            ServerAudioOutputPdu::Volume(VolumePdu {
                volume_left: 0,
                volume_right: 0,
            }),
        );
        dvc_audio(
            &mut client,
            1,
            ServerAudioOutputPdu::Pitch(PitchPdu { pitch: 0x2_0000 }),
        );
        dvc_audio(&mut client, 1, wave(1, &[7, -8]));
        // An incomplete stereo frame and an unnegotiated format are discarded.
        dvc_audio(&mut client, 1, wave(1, &[9]));
        dvc_audio(&mut client, 1, wave(9, &[10, -11]));
        dvc_audio(&mut client, 1, ServerAudioOutputPdu::Close);
        dvc_audio(&mut client, 1, wave(1, &[12, -13]));
        assert_eq!(
            *audio.calls.lock().unwrap(),
            vec![
                AudioCall::Format(48_000, 2),
                AudioCall::Samples(vec![1, -2]),
                AudioCall::Samples(vec![3, -4, 5, -6]),
                AudioCall::Format(44_100, 2),
                AudioCall::Samples(vec![7, -8]),
                AudioCall::Stop,
                AudioCall::Format(44_100, 2),
                AudioCall::Samples(vec![12, -13]),
            ]
        );
    })
    .await;
}
