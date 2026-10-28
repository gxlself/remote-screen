//! Worker behavior with deterministic fake backends; no native codec is loaded.

use std::collections::BTreeMap;
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use host_core::encode_model::{EncParams, RateCost};
use host_core::platform::{
    CapturedFrame, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, PlatformError,
};
use host_linux::enc_worker::{
    BackendError, BackendFactory, EncBackend, EncPacket, EncWorker, SrcSize, DROP_WAIT,
};
use host_linux::frame::{FramePool, LinuxFrame};
use rsp_core::pb::Codec;
use tokio::sync::mpsc as tmpsc;

const WAIT: Duration = Duration::from_secs(5);
const SPS_PPS: &[u8] = &[0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Open {
        name: &'static str,
        bitrate_bps: u64,
        src: SrcSize,
    },
    Encode {
        name: &'static str,
        pts: i64,
        idr: bool,
        generation: u64,
        src: SrcSize,
        marker: u8,
    },
    SetRate {
        name: &'static str,
        bps: u64,
        fps: u32,
    },
    Drop {
        name: &'static str,
    },
}

struct Recorder {
    calls: Arc<Mutex<Vec<Call>>>,
    notices: mpsc::Sender<Call>,
}

impl Recorder {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call.clone());
        let _ = self.notices.send(call);
    }
}

struct Log {
    recorder: Arc<Recorder>,
    notices: mpsc::Receiver<Call>,
}

impl Log {
    fn new() -> Self {
        let (tx, notices) = mpsc::channel();
        Self {
            recorder: Arc::new(Recorder {
                calls: Arc::new(Mutex::new(Vec::new())),
                notices: tx,
            }),
            notices,
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.recorder.calls.lock().unwrap().clone()
    }

    fn wait(&self, predicate: impl Fn(&Call) -> bool) -> Call {
        loop {
            let call = self.notices.recv_timeout(WAIT).expect("backend call");
            if predicate(&call) {
                return call;
            }
        }
    }

    fn encodes(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| matches!(call, Call::Encode { .. }))
            .collect()
    }

    fn opens(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| matches!(call, Call::Open { .. }))
            .collect()
    }

    fn rates(&self) -> Vec<u64> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::SetRate { bps, .. } => Some(bps),
                _ => None,
            })
            .collect()
    }
}

struct Block {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

#[derive(Default)]
struct Script {
    opens: usize,
    encodes: usize,
    rates: usize,
    open_errors: BTreeMap<usize, BackendError>,
    encode_errors: BTreeMap<usize, BackendError>,
    rate_errors: BTreeMap<usize, BackendError>,
    blocks: BTreeMap<usize, Block>,
    delay_output: bool,
    unflagged_keys: bool,
    ignore_idr: usize,
}

struct FakeFactory {
    name: &'static str,
    rate_cost: RateCost,
    script: Arc<Mutex<Script>>,
    log: Arc<Recorder>,
    completed: mpsc::Sender<usize>,
}

struct Fake {
    factory: Arc<FakeFactory>,
    completed: mpsc::Receiver<usize>,
}

impl Fake {
    fn new(log: &Log, name: &'static str, rate_cost: RateCost) -> Self {
        let (tx, completed) = mpsc::channel();
        Self {
            factory: Arc::new(FakeFactory {
                name,
                rate_cost,
                script: Arc::new(Mutex::new(Script::default())),
                log: Arc::clone(&log.recorder),
                completed: tx,
            }),
            completed,
        }
    }

    fn chain(&self) -> Vec<Arc<dyn BackendFactory>> {
        vec![self.factory.clone()]
    }

    fn script(&self, change: impl FnOnce(&mut Script)) {
        change(&mut self.factory.script.lock().unwrap());
    }

    fn block(&self, nth: usize) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, entered_rx) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        self.script(|script| {
            script.blocks.insert(nth, Block { entered, release });
        });
        (entered_rx, release_tx)
    }

    fn done(&self, nth: usize) {
        assert_eq!(self.completed.recv_timeout(WAIT).unwrap(), nth);
    }
}

impl BackendFactory for FakeFactory {
    fn name(&self) -> &'static str {
        self.name
    }

    fn open(&self, params: &EncParams, src: SrcSize) -> Result<Box<dyn EncBackend>, BackendError> {
        self.log.record(Call::Open {
            name: self.name,
            bitrate_bps: params.bitrate_bps,
            src,
        });
        let mut script = self.script.lock().unwrap();
        script.opens += 1;
        if let Some(error) = script.open_errors.get(&script.opens) {
            return Err(error.clone());
        }
        drop(script);
        Ok(Box::new(FakeBackend {
            name: self.name,
            rate_cost: self.rate_cost,
            script: Arc::clone(&self.script),
            log: Arc::clone(&self.log),
            completed: self.completed.clone(),
            src,
            codec: params.codec,
            first_idr: true,
            pending: None,
        }))
    }
}

struct FakeBackend {
    name: &'static str,
    rate_cost: RateCost,
    script: Arc<Mutex<Script>>,
    log: Arc<Recorder>,
    completed: mpsc::Sender<usize>,
    src: SrcSize,
    codec: Codec,
    first_idr: bool,
    pending: Option<EncPacket>,
}

impl EncBackend for FakeBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn rate_cost(&self) -> RateCost {
        self.rate_cost
    }

    fn encode(
        &mut self,
        frame: &LinuxFrame,
        pts: i64,
        idr: bool,
    ) -> Result<Vec<EncPacket>, BackendError> {
        let src = SrcSize {
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
        };
        assert_eq!(
            src, self.src,
            "worker must reopen for changed input dimensions"
        );
        self.log.record(Call::Encode {
            name: self.name,
            pts,
            idr,
            generation: frame.generation,
            src,
            marker: frame.pixels[0],
        });
        let (nth, block, error, delay, unflagged, actual_idr) = {
            let mut script = self.script.lock().unwrap();
            script.encodes += 1;
            let nth = script.encodes;
            let block = script.blocks.remove(&nth);
            let error = script.encode_errors.get(&nth).cloned();
            let actual_idr = idr && nth > script.ignore_idr;
            (
                nth,
                block,
                error,
                script.delay_output,
                script.unflagged_keys,
                actual_idr,
            )
        };
        if let Some(block) = block {
            block.entered.send(()).unwrap();
            block
                .release
                .recv_timeout(WAIT)
                .expect("release fake encoder");
        }
        let result = match error {
            Some(error) => Err(error),
            None => {
                let mut data = Vec::new();
                if self.codec == Codec::Hevc {
                    if actual_idr && self.first_idr {
                        for nal_type in [32, 33, 34] {
                            data.extend_from_slice(&[0, 0, 0, 1, nal_type << 1, 1, 2]);
                        }
                    }
                    data.extend_from_slice(&[0, 0, 0, 1, if actual_idr { 19 << 1 } else { 2 }, 1]);
                } else {
                    if actual_idr && self.first_idr {
                        data.extend_from_slice(SPS_PPS);
                    }
                    data.extend_from_slice(&[0, 0, 0, 1, if actual_idr { 0x65 } else { 0x41 }]);
                }
                self.first_idr &= !actual_idr;
                data.extend_from_slice(&[self.name.as_bytes()[0], frame.pixels[0]]);
                let packet = EncPacket {
                    data,
                    pts,
                    keyframe: actual_idr && !unflagged,
                };
                Ok(if delay {
                    self.pending.replace(packet).into_iter().collect()
                } else {
                    vec![packet]
                })
            }
        };
        let _ = self.completed.send(nth);
        result
    }

    fn set_rate(&mut self, bps: u64, fps: u32) -> Result<(), BackendError> {
        self.log.record(Call::SetRate {
            name: self.name,
            bps,
            fps,
        });
        let mut script = self.script.lock().unwrap();
        script.rates += 1;
        match script.rate_errors.get(&script.rates) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.log.record(Call::Drop { name: self.name });
    }
}

/// Pull output on demand so capacity-one sinks really stay full until the test
/// reads. The main test only waits with recv_timeout; shutdown closes the sink
/// and releases a blocked reader even if the main test panics.
struct Output {
    request: mpsc::Sender<()>,
    frames: mpsc::Receiver<Option<EncodedFrame>>,
}

impl Output {
    fn new(capacity: usize) -> (tmpsc::Sender<EncodedFrame>, Self) {
        let (sink, mut rx) = tmpsc::channel(capacity);
        let (request, requests) = mpsc::channel();
        let (tx, frames) = mpsc::channel();
        thread::spawn(move || {
            while requests.recv_timeout(WAIT).is_ok() {
                let frame = rx.blocking_recv();
                let closed = frame.is_none();
                if tx.send(frame).is_err() || closed {
                    break;
                }
            }
        });
        (sink, Self { request, frames })
    }

    fn recv(&self) -> EncodedFrame {
        self.request.send(()).unwrap();
        self.frames
            .recv_timeout(WAIT)
            .expect("encoder output within timeout")
            .expect("encoder output channel is open")
    }

    fn closed(&self) {
        self.request.send(()).unwrap();
        assert!(self.frames.recv_timeout(WAIT).unwrap().is_none());
    }
}

struct Frames(FramePool);

impl Frames {
    fn new() -> Self {
        Self(FramePool::new(16))
    }

    fn frame(&mut self, marker: u8) -> CapturedFrame {
        self.sized(marker, 1, 640, 360, 640 * 4)
    }

    fn sized(
        &mut self,
        marker: u8,
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
    ) -> CapturedFrame {
        let native = self
            .0
            .acquire(width, height, stride, generation, |pixels| {
                pixels[0] = marker
            })
            .unwrap();
        CapturedFrame {
            display_id: 1,
            width,
            height,
            capture_ts_us: 1_000_000 + u64::from(marker),
            native,
        }
    }
}

fn config(sink: tmpsc::Sender<EncodedFrame>) -> EncoderConfig {
    EncoderConfig {
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 8000,
        sink,
    }
}

fn unsupported() -> BackendError {
    BackendError::Unsupported("fake unsupported".into())
}

fn failed() -> BackendError {
    BackendError::Failed("fake failure".into())
}

fn submit(worker: &mut EncWorker, frames: &mut Frames, marker: u8) {
    worker
        .encode(frames.frame(marker), EncodeOpts::default())
        .unwrap();
}

fn markers(log: &Log) -> Vec<u8> {
    log.encodes()
        .into_iter()
        .map(|call| match call {
            Call::Encode { marker, .. } => marker,
            _ => unreachable!(),
        })
        .collect()
}

fn idrs(log: &Log) -> Vec<bool> {
    log.encodes()
        .into_iter()
        .map(|call| match call {
            Call::Encode { idr, .. } => idr,
            _ => unreachable!(),
        })
        .collect()
}

#[test]
fn start_validates() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, _output) = Output::new(4);
    let valid = config(sink);
    let mut cfg = valid.clone();
    cfg.codec = Codec::Unspecified;
    assert!(matches!(
        EncWorker::start(cfg, fake.chain(), None),
        Err(PlatformError::Unsupported)
    ));
    let mut cfg = valid.clone();
    cfg.width = 64;
    cfg.height = 64;
    assert!(matches!(
        EncWorker::start(cfg, fake.chain(), None),
        Err(PlatformError::Unsupported)
    ));
    let mut cfg = valid.clone();
    cfg.fps = 0;
    assert!(matches!(
        EncWorker::start(cfg, fake.chain(), None),
        Err(PlatformError::Platform(_))
    ));
    assert!(matches!(
        EncWorker::start(valid, vec![], None),
        Err(PlatformError::Unsupported)
    ));
    assert!(log.calls().is_empty());
}

#[test]
fn probe_falls_through() {
    let log = Log::new();
    let a = Fake::new(&log, "A", RateCost::Free);
    let b = Fake::new(&log, "B", RateCost::Free);
    a.script(|script| {
        script.open_errors.insert(1, unsupported());
    });
    let (sink, _output) = Output::new(4);
    let worker = EncWorker::start(
        config(sink),
        vec![a.factory.clone(), b.factory.clone()],
        None,
    )
    .unwrap();
    assert_eq!(worker.status().backend, Some("B"));
    assert_eq!(log.opens().len(), 2);
    drop(worker);

    let a = Fake::new(&log, "A", RateCost::Free);
    let b = Fake::new(&log, "B", RateCost::Free);
    for fake in [&a, &b] {
        fake.script(|script| {
            script.open_errors.insert(1, unsupported());
        });
    }
    let (sink, _output) = Output::new(4);
    assert!(matches!(
        EncWorker::start(config(sink), vec![a.factory, b.factory], None),
        Err(PlatformError::Unsupported)
    ));
}

#[test]
fn rejects_foreign_frame() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, _output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let frame = CapturedFrame {
        display_id: 1,
        width: 640,
        height: 360,
        capture_ts_us: 7,
        native: Arc::new(5u32),
    };
    assert!(
        matches!(worker.encode(frame, EncodeOpts::default()), Err(PlatformError::Platform(message)) if message == "not a Linux capture frame")
    );
    assert_eq!(worker.status().stats.submitted, 0);
}

#[test]
fn first_frame_is_idr() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    let encoded = output.recv();
    assert_eq!(idrs(&log), vec![true]);
    assert!(encoded.keyframe);
    assert!(!encoded.still_refine);
    assert!(encoded.data.starts_with(SPS_PPS));
    assert_eq!(encoded.capture_ts_us, 1_000_001);
    assert!(encoded.encoded_ts_us > encoded.capture_ts_us);
    assert_eq!(log.opens().len(), 1, "reuse successful probe");
}

#[test]
fn force_keyframe_paths() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.force_keyframe();
    submit(&mut worker, &mut frames, 2);
    assert!(output.recv().keyframe);
    worker
        .encode(
            frames.frame(3),
            EncodeOpts {
                force_keyframe: true,
                quality_boost: false,
            },
        )
        .unwrap();
    assert!(output.recv().keyframe);
    assert_eq!(idrs(&log), vec![true, true, true]);
}

#[test]
fn mailbox_latest_wins() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (entered, release) = fake.block(1);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    entered.recv_timeout(WAIT).unwrap();
    for marker in 2..=4 {
        submit(&mut worker, &mut frames, marker);
    }
    assert_eq!(worker.status().stats.mailbox_dropped, 2);
    release.send(()).unwrap();
    assert_eq!(output.recv().capture_ts_us, 1_000_001);
    assert_eq!(output.recv().capture_ts_us, 1_000_004);
    assert_eq!(markers(&log), vec![1, 4]);
    assert_eq!(worker.status().stats.submitted, 4);
    assert_eq!(worker.status().stats.mailbox_dropped, 2);
}

#[test]
fn param_sets_prepended() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.force_keyframe();
    submit(&mut worker, &mut frames, 2);
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert!(encoded.data.starts_with(SPS_PPS));
    assert_eq!(
        &encoded.data[SPS_PPS.len()..SPS_PPS.len() + 5],
        &[0, 0, 0, 1, 0x65]
    );
}

#[test]
fn delayed_output_maps_pts() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| script.delay_output = true);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    fake.done(1);
    submit(&mut worker, &mut frames, 2);
    assert_eq!(output.recv().capture_ts_us, 1_000_001);
    fake.done(2);
    submit(&mut worker, &mut frames, 3);
    let encoded = output.recv();
    assert_eq!(encoded.capture_ts_us, 1_000_002);
    assert!(!encoded.keyframe);
}

#[test]
fn sink_full_drops_and_rekeys() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (entered, release) = fake.block(4);
    let (sink, output) = Output::new(1);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    for marker in 1..=3 {
        submit(&mut worker, &mut frames, marker);
        fake.done(usize::from(marker));
    }
    submit(&mut worker, &mut frames, 4);
    // Entering the next backend call proves the previous delivery completed.
    // Keep that call blocked while reading the sole queued packet.
    entered.recv_timeout(WAIT).unwrap();
    assert_eq!(worker.status().stats.sink_dropped, 2);
    assert_eq!(output.recv().capture_ts_us, 1_000_001);
    release.send(()).unwrap();
    let encoded = output.recv();
    assert_eq!(encoded.capture_ts_us, 1_000_004);
    assert!(encoded.keyframe);
    assert_eq!(idrs(&log), vec![true, false, true, true]);
    assert_eq!(worker.status().stats.sink_dropped, 2);
}

#[test]
fn rate_free() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    worker.set_rate(4000, 30);
    submit(&mut worker, &mut frames, 1);
    output.recv();
    assert_eq!(log.rates(), vec![4_000_000]);
    worker.set_rate(3990, 30);
    submit(&mut worker, &mut frames, 2);
    output.recv();
    assert_eq!(log.rates(), vec![4_000_000]);
    assert_eq!(worker.status().stats.rate_applied, 1);
}

#[test]
fn rate_idr() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Idr);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.set_rate(4000, 30);
    submit(&mut worker, &mut frames, 2);
    assert!(output.recv().keyframe);
    assert_eq!(log.rates(), vec![4_000_000]);
    assert_eq!(idrs(&log), vec![true, true]);
}

#[test]
fn rate_reopen() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Reopen);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.set_rate(4000, 30);
    submit(&mut worker, &mut frames, 2);
    assert!(output.recv().keyframe);
    assert_eq!(log.opens().len(), 2);
    assert!(matches!(
        log.opens()[1],
        Call::Open {
            bitrate_bps: 4_000_000,
            ..
        }
    ));
    assert_eq!(idrs(&log), vec![true, true]);
    assert!(log.rates().is_empty());
    assert_eq!(worker.status().stats.reopens, 1);
    assert_eq!(worker.status().stats.rate_applied, 1);
}

#[test]
fn source_change_reopens() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker
        .encode(frames.sized(2, 2, 640, 360, 2560), EncodeOpts::default())
        .unwrap();
    assert!(output.recv().keyframe);
    worker
        .encode(frames.sized(3, 2, 1280, 720, 5120), EncodeOpts::default())
        .unwrap();
    assert!(output.recv().keyframe);
    assert_eq!(log.opens().len(), 3);
    assert!(matches!(
        log.opens()[1],
        Call::Open {
            src: SrcSize {
                width: 640,
                height: 360,
                stride: 2560
            },
            ..
        }
    ));
    assert!(matches!(
        log.opens()[2],
        Call::Open {
            src: SrcSize {
                width: 1280,
                height: 720,
                stride: 5120
            },
            ..
        }
    ));
    assert_eq!(worker.status().stats.reopens, 2);
}

#[test]
fn runtime_failure_backoff() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| {
        script.encode_errors.insert(2, failed());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    fake.done(1);
    submit(&mut worker, &mut frames, 2);
    fake.done(2);
    log.wait(|call| matches!(call, Call::Drop { name: "A" }));
    assert_eq!(worker.status().stats.failures, 1);
    submit(&mut worker, &mut frames, 3);
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert_eq!(encoded.capture_ts_us, 1_000_003);
    assert_eq!(log.opens().len(), 2);
    assert_eq!(markers(&log), vec![1, 2, 3]);
}

#[test]
fn runtime_unsupported_moves_on() {
    let log = Log::new();
    let a = Fake::new(&log, "A", RateCost::Free);
    let b = Fake::new(&log, "B", RateCost::Free);
    a.script(|script| {
        script.encode_errors.insert(2, unsupported());
    });
    let (sink, output) = Output::new(4);
    let mut worker =
        EncWorker::start(config(sink), vec![a.factory.clone(), b.factory], None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    a.done(1);
    submit(&mut worker, &mut frames, 2);
    a.done(2);
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert_eq!(encoded.data[encoded.data.len() - 2], b'B');
    assert_eq!(worker.status().backend, Some("B"));
    assert_eq!(worker.status().stats.failures, 0);
}

#[test]
fn chain_exhausted_is_dead() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| {
        script.encode_errors.insert(2, unsupported());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    submit(&mut worker, &mut frames, 2);
    output.closed();
    assert_eq!(worker.status().dead.as_deref(), Some("no usable encoder"));
    assert!(
        matches!(worker.encode(frames.frame(3), EncodeOpts::default()), Err(PlatformError::Platform(message)) if message == "no usable encoder")
    );
}

#[test]
fn idle_key_retry() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.force_keyframe();
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert_eq!(encoded.capture_ts_us, 1_000_001);
    assert_eq!(worker.status().stats.idle_key_retries, 1);
    assert_eq!(markers(&log), vec![1, 1]);
    assert_eq!(idrs(&log), vec![true, true]);
    let calls = log.encodes();
    match (&calls[0], &calls[1]) {
        (
            Call::Encode {
                generation: a,
                src: sa,
                ..
            },
            Call::Encode {
                generation: b,
                src: sb,
                ..
            },
        ) => {
            assert_eq!(a, b);
            assert_eq!(sa, sb);
        }
        _ => unreachable!(),
    }
}

/// Observe the exact release of the formerly retained capture. This confirms
/// a skipped refine has been consumed without ordering the test with sleeps.
fn released(weak: Weak<dyn std::any::Any + Send + Sync>) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let deadline = Instant::now() + WAIT;
        while weak.strong_count() != 0 && Instant::now() < deadline {
            thread::yield_now();
        }
        let _ = tx.send(weak.strong_count() == 0);
    });
    assert!(rx.recv_timeout(WAIT).unwrap(), "previous capture released");
}

#[test]
fn refine_rules() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.set_quality_boost(true);
    let frame = frames.frame(2);
    let weak = Arc::downgrade(&frame.native);
    worker
        .encode(
            frame,
            EncodeOpts {
                quality_boost: true,
                force_keyframe: false,
            },
        )
        .unwrap();
    let refined = output.recv();
    assert!(refined.still_refine);
    assert!(!refined.keyframe);
    frames.0.clear();
    worker.set_quality_boost(false);
    worker
        .encode(
            frames.frame(3),
            EncodeOpts {
                quality_boost: true,
                force_keyframe: false,
            },
        )
        .unwrap();
    released(weak);
    assert_eq!(markers(&log), vec![1, 2], "disabled refine skips backend");
    worker
        .encode(
            frames.frame(4),
            EncodeOpts {
                quality_boost: true,
                force_keyframe: true,
            },
        )
        .unwrap();
    let key = output.recv();
    assert!(key.keyframe);
    assert!(!key.still_refine);
    assert_eq!(markers(&log), vec![1, 2, 4]);
}

#[test]
fn closed_sink_kills() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, receiver) = tmpsc::channel(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    drop(receiver);
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    fake.done(1);
    // Backend destruction follows the sticky error publication.
    log.wait(|call| matches!(call, Call::Drop { .. }));
    assert_eq!(
        worker.status().dead.as_deref(),
        Some("encoder output closed")
    );
    assert!(
        matches!(worker.encode(frames.frame(2), EncodeOpts::default()), Err(PlatformError::Platform(message)) if message == "encoder output closed")
    );
}

#[test]
fn drop_joins_worker() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, _output) = Output::new(4);
    let worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let started = Instant::now();
    drop(worker);
    assert!(started.elapsed() < DROP_WAIT);
    assert_eq!(log.calls().last(), Some(&Call::Drop { name: "A" }));
}

#[test]
fn failed_probe_is_excluded_from_runtime_chain() {
    let log = Log::new();
    let a = Fake::new(&log, "A", RateCost::Free);
    let b = Fake::new(&log, "B", RateCost::Free);
    a.script(|script| {
        script.open_errors.insert(1, failed());
    });
    b.script(|script| {
        script.encode_errors.insert(2, unsupported());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), vec![a.factory, b.factory], None).unwrap();
    assert_eq!(worker.status().backend, Some("B"));
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    submit(&mut worker, &mut frames, 2);
    output.closed();
    assert_eq!(worker.status().dead.as_deref(), Some("no usable encoder"));
    assert_eq!(log.opens().len(), 2);
}

#[test]
fn fallback_skips_failed_and_unsupported_opens() {
    let log = Log::new();
    let a = Fake::new(&log, "A", RateCost::Free);
    let b = Fake::new(&log, "B", RateCost::Free);
    let c = Fake::new(&log, "C", RateCost::Free);
    let d = Fake::new(&log, "D", RateCost::Free);
    a.script(|script| {
        script.encode_errors.insert(2, unsupported());
    });
    b.script(|script| {
        script.open_errors.insert(1, failed());
    });
    c.script(|script| {
        script.open_errors.insert(1, unsupported());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(
        config(sink),
        vec![a.factory, b.factory, c.factory, d.factory],
        None,
    )
    .unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    submit(&mut worker, &mut frames, 2);
    let encoded = output.recv();
    assert_eq!(encoded.data[encoded.data.len() - 2], b'D');
    assert_eq!(worker.status().backend, Some("D"));
    assert_eq!(worker.status().stats.failures, 0);
    worker
        .encode(frames.sized(3, 2, 640, 360, 2560), EncodeOpts::default())
        .unwrap();
    output.recv();
    let names: Vec<_> = log
        .opens()
        .into_iter()
        .map(|call| match call {
            Call::Open { name, .. } => name,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(names, vec!["A", "B", "C", "D", "D"]);
}

#[test]
fn pending_frames_stay_latest_during_failure_backoff() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| {
        script.encode_errors.insert(2, failed());
    });
    let (entered, release) = fake.block(2);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    submit(&mut worker, &mut frames, 2);
    entered.recv_timeout(WAIT).unwrap();
    for marker in 3..=5 {
        submit(&mut worker, &mut frames, marker);
    }
    release.send(()).unwrap();
    let encoded = output.recv();
    assert_eq!(encoded.capture_ts_us, 1_000_005);
    assert!(encoded.keyframe);
    assert_eq!(markers(&log), vec![1, 2, 5]);
    assert_eq!(worker.status().stats.mailbox_dropped, 2);
    assert_eq!(worker.status().stats.failures, 1);
}

#[test]
fn failed_rate_change_preserves_pending_frame_and_bitrate() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Idr);
    fake.script(|script| {
        script.rate_errors.insert(1, failed());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    worker.set_rate(4000, 30);
    submit(&mut worker, &mut frames, 2);
    let encoded = output.recv();
    assert_eq!(encoded.capture_ts_us, 1_000_002);
    assert!(encoded.keyframe);
    assert!(matches!(
        log.opens()[1],
        Call::Open {
            bitrate_bps: 4_000_000,
            ..
        }
    ));
    assert_eq!(worker.status().stats.failures, 1);
    assert_eq!(worker.status().stats.rate_applied, 1);
}

#[test]
fn repeated_runtime_failures_retry_without_new_capture() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| {
        script.encode_errors.insert(2, failed());
        script.encode_errors.insert(3, failed());
    });
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    output.recv();
    submit(&mut worker, &mut frames, 2);
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert_eq!(encoded.capture_ts_us, 1_000_002);
    assert_eq!(log.opens().len(), 3);
    assert_eq!(worker.status().stats.failures, 2);
}

#[test]
fn larger_first_source_and_stride_changes_reopen() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    worker
        .encode(frames.sized(1, 1, 1280, 720, 5120), EncodeOpts::default())
        .unwrap();
    assert!(output.recv().keyframe);
    worker
        .encode(frames.sized(2, 1, 1280, 720, 5184), EncodeOpts::default())
        .unwrap();
    assert!(output.recv().keyframe);
    assert_eq!(log.opens().len(), 3);
    assert!(matches!(
        log.opens()[2],
        Call::Open {
            src: SrcSize {
                width: 1280,
                height: 720,
                stride: 5184
            },
            ..
        }
    ));
    assert_eq!(worker.status().stats.reopens, 2);
}

#[test]
fn nal_key_detection_handles_h264_and_hevc() {
    for codec in [Codec::H264, Codec::Hevc] {
        let log = Log::new();
        let fake = Fake::new(&log, "A", RateCost::Free);
        fake.script(|script| script.unflagged_keys = true);
        let (sink, output) = Output::new(4);
        let mut cfg = config(sink);
        cfg.codec = codec;
        let mut worker = EncWorker::start(cfg, fake.chain(), None).unwrap();
        let mut frames = Frames::new();
        submit(&mut worker, &mut frames, 1);
        let first = output.recv();
        assert!(first.keyframe);
        worker.force_keyframe();
        submit(&mut worker, &mut frames, 2);
        let second = output.recv();
        assert!(second.keyframe);
        // Only the first backend IDR includes parameter sets. Restoring them
        // makes the prefix of both outputs identical for both codecs.
        let prefix_len = if codec == Codec::Hevc {
            21
        } else {
            SPS_PPS.len()
        };
        assert_eq!(&first.data[..prefix_len], &second.data[..prefix_len]);
    }
}

#[test]
fn output_gate_rejects_nonkey_after_reopen() {
    let log = Log::new();
    let fake = Fake::new(&log, "A", RateCost::Free);
    fake.script(|script| script.ignore_idr = 1);
    let (sink, output) = Output::new(4);
    let mut worker = EncWorker::start(config(sink), fake.chain(), None).unwrap();
    let mut frames = Frames::new();
    submit(&mut worker, &mut frames, 1);
    // The first fake packet ignores IDR. Gating requests an idle key retry.
    let encoded = output.recv();
    assert!(encoded.keyframe);
    assert_eq!(worker.status().stats.gated, 1);
    assert_eq!(worker.status().stats.idle_key_retries, 1);
    assert_eq!(idrs(&log), vec![true, true]);
}
