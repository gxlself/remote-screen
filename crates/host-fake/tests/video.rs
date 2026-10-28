use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, ClipboardItem, ConsoleState, CursorUpdate,
    DisplayInfo, EncodeOpts, EncodedFrame, Encoder, EncoderConfig, FrameSink, HostInfo, InputEvent,
    KeyStore, Nic, PermissionKind, Permissions, Platform, Result as PlatformResult,
    SessionActionKind,
};
use host_core::{HostRuntime, RuntimeOptions};
use host_fake::FakePlatform;
use rsp_client::{
    EventStream, SessionEvent as ClientEvent, SessionHandle, SessionParams, StartParams,
};
use rsp_core::clock::unix_micros;
use rsp_core::fec;
use rsp_core::pb;
use rsp_core::video::{packetize, FrameMeta, Reassembler, ReassemblyEvent, FLAG_KEYFRAME};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
mod common;

const WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const WARMUP_FRAMES: usize = 10;
const MIN_FRAMES: usize = 20;
const FRAMES_AFTER_RECOVERY: usize = 10;
const TINY_FRAME_BYTES: usize = 256;
// Video DATAGRAMs are close to the path MTU, while ACK/control packets are
// much smaller after streaming starts. Do not inject faults into handshake or
// control traffic.
const VIDEO_PACKET_MIN: usize = 900;

#[tokio::test]
async fn fake_encoder_packet_loss_recovers_with_fec() {
    let platform = FakePlatform::new();
    let (tx, _rx) = mpsc::channel(2);
    let _encoder = platform
        .encoder(EncoderConfig {
            codec: pb::Codec::Hevc,
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_kbps: 2_000,
            sink: tx,
        })
        .unwrap();
    let frame = vec![0x42; 3_000];
    let packets = packetize(
        &frame,
        FrameMeta {
            flags: FLAG_KEYFRAME,
            display_id: 1,
            epoch: 1,
            frame_id: 0,
            capture_ts_us: 1,
        },
        1200,
        0.5,
    )
    .unwrap();
    let (first, _) = rsp_core::video::VideoHeader::parse(&packets[0]).unwrap();
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    for (idx, packet) in packets.iter().enumerate() {
        if idx == 0 {
            continue;
        }
        events.extend(reassembler.push(packet, 1_000).unwrap());
    }
    let recovered = events.into_iter().find_map(|event| match event {
        ReassemblyEvent::Frame(frame) => Some(frame),
        _ => None,
    });
    assert_eq!(
        recovered.as_ref().map(|f| f.data.as_slice()),
        Some(frame.as_slice())
    );
    assert!(first.parity_shards > 0);
}

/// Encodes deliberately small access units so the production packetizer always
/// emits one data shard followed by its parity shards. Keeping the frame below
/// the smallest QUIC DATAGRAM shard makes the number of datagrams per frame
/// independent of path-MTU growth, and therefore makes datagram-loss tests
/// deterministic without changing the production packetizer.
struct TinyFramePlatform {
    inner: FakePlatform,
}

impl TinyFramePlatform {
    fn new() -> Self {
        Self {
            inner: FakePlatform::new(),
        }
    }

    fn host_fingerprint(&self) -> [u8; 32] {
        common::host_fingerprint(&self.inner)
    }
}

impl Platform for TinyFramePlatform {
    fn info(&self) -> HostInfo {
        self.inner.info()
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.inner.displays()
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        self.inner.watch_displays(tx);
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> PlatformResult<Box<dyn CaptureHandle>> {
        self.inner.capture(display, cfg, sink)
    }

    fn encoder(&self, cfg: EncoderConfig) -> PlatformResult<Box<dyn Encoder>> {
        Ok(Box::new(TinyFrameEncoder {
            cfg,
            force: true,
            quality_boost: false,
            last_key_us: 0,
        }))
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.inner.inject(ev, display);
    }

    fn inject_input(&self, msg: &rsp_core::hid::InputMsg, display: &DisplayInfo) {
        self.inner.inject_input(msg, display);
    }

    fn release_all(&self) {
        self.inner.release_all();
    }

    fn session_action(&self, kind: SessionActionKind) -> PlatformResult<()> {
        self.inner.session_action(kind)
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        self.inner.watch_cursor(tx);
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        self.inner.watch_local_activity(tx);
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.inner.clipboard_get()
    }

    fn clipboard_set(&self, item: ClipboardItem) -> PlatformResult<()> {
        self.inner.clipboard_set(item)
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        self.inner.watch_clipboard(tx);
    }

    fn power_keep_awake(&self, on: bool) {
        self.inner.power_keep_awake(on);
    }

    fn console_state(&self) -> ConsoleState {
        self.inner.console_state()
    }

    fn screen_sharing_enabled(&self) -> Option<bool> {
        self.inner.screen_sharing_enabled()
    }

    fn filevault_enabled(&self) -> Option<bool> {
        self.inner.filevault_enabled()
    }

    fn permissions(&self) -> Permissions {
        self.inner.permissions()
    }

    fn request_permission(&self, kind: PermissionKind) {
        self.inner.request_permission(kind);
    }

    fn nics(&self) -> Vec<Nic> {
        self.inner.nics()
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> PlatformResult<()> {
        self.inner.send_wol(mac, via)
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.inner.keystore()
    }
}

struct TinyFrameEncoder {
    cfg: EncoderConfig,
    force: bool,
    quality_boost: bool,
    last_key_us: u64,
}

impl Encoder for TinyFrameEncoder {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> PlatformResult<()> {
        let keyframe = self.force
            || opts.force_keyframe
            || frame.capture_ts_us.saturating_sub(self.last_key_us) >= 2_000_000;
        let mut data = Vec::with_capacity(TINY_FRAME_BYTES);
        data.extend_from_slice(&[0, 0, 0, 1, if keyframe { 19 << 1 } else { 1 << 1 }, 1]);
        data.resize(
            TINY_FRAME_BYTES,
            if self.quality_boost || opts.quality_boost {
                0x66
            } else {
                0x55
            },
        );
        let output = EncodedFrame {
            data,
            keyframe,
            still_refine: self.quality_boost || opts.quality_boost,
            capture_ts_us: frame.capture_ts_us,
            encoded_ts_us: unix_micros(),
        };
        match self.cfg.sink.try_send(output) {
            Ok(()) => {
                self.force = false;
                if keyframe {
                    self.last_key_us = frame.capture_ts_us;
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => self.force = true,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return Err(host_core::PlatformError::Platform(
                    "tiny encoder sink closed".into(),
                ));
            }
        }
        Ok(())
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        self.cfg.bitrate_kbps = kbps;
        self.cfg.fps = fps;
    }

    fn force_keyframe(&mut self) {
        self.force = true;
    }

    fn set_quality_boost(&mut self, on: bool) {
        self.quality_boost = on;
    }
}

struct RunningHost {
    endpoint: quinn::Endpoint,
    task: JoinHandle<host_core::RuntimeResult<()>>,
    addr: SocketAddr,
    host_fp: [u8; 32],
}

impl RunningHost {
    fn start<P: Platform>(platform: P, host_fp: [u8; 32]) -> Self {
        let (endpoint, _) = host_core::listen::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = endpoint.local_addr().unwrap();
        let runtime = HostRuntime::new(
            platform,
            RuntimeOptions {
                bind: Some(addr),
                view_only: false,
                dump: None,
                ..RuntimeOptions::default()
            },
        )
        .with_store(common::paired_store());
        let task = tokio::spawn(runtime.serve(endpoint.clone()));
        Self {
            endpoint,
            task,
            addr,
            host_fp,
        }
    }
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test complete");
        self.task.abort();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum DropMode {
    Idle = 0,
    Sparse = 1,
    Burst = 2,
}

struct DropControl {
    mode: AtomicU8,
    burst_left: AtomicU64,
    dropped: AtomicU64,
    video_packets: AtomicU64,
}

struct DropProxy {
    addr: SocketAddr,
    task: JoinHandle<()>,
    control: Arc<DropControl>,
}

impl DropProxy {
    async fn start(host_addr: SocketAddr) -> Self {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = socket.local_addr().unwrap();
        let task_socket = Arc::clone(&socket);
        let control = Arc::new(DropControl {
            mode: AtomicU8::new(DropMode::Idle as u8),
            burst_left: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            video_packets: AtomicU64::new(0),
        });
        let task_control = Arc::clone(&control);
        let task = tokio::spawn(async move {
            let mut buffer = vec![0u8; 64 * 1024];
            let mut client_addr = None;
            let mut sparse_packets = 0u64;
            let mut sparse_index = 0u64;
            loop {
                let Ok((size, source)) = task_socket.recv_from(&mut buffer).await else {
                    return;
                };
                if source == host_addr {
                    let Some(client) = client_addr else {
                        continue;
                    };
                    let video_like = size >= VIDEO_PACKET_MIN;
                    if video_like {
                        task_control.video_packets.fetch_add(1, Ordering::Relaxed);
                    }
                    let mode = task_control.mode.load(Ordering::Acquire);
                    let drop = match mode {
                        mode if mode == DropMode::Sparse as u8 && video_like => {
                            sparse_packets += 1;
                            // Alternate 19 and 21 packet gaps: the average is
                            // 5%, and the odd gaps alternate the shard position
                            // in the two-datagram frame. A fixed 20-packet gap
                            // could repeatedly hit only parity, never exercising
                            // FEC recovery.
                            let interval = if sparse_index.is_multiple_of(2) {
                                19
                            } else {
                                21
                            };
                            if sparse_packets >= interval {
                                sparse_packets = 0;
                                sparse_index = sparse_index.wrapping_add(1);
                                true
                            } else {
                                false
                            }
                        }
                        mode if mode == DropMode::Burst as u8 && video_like => task_control
                            .burst_left
                            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                                left.checked_sub(1)
                            })
                            .is_ok_and(|previous| previous > 0),
                        _ => false,
                    };
                    if drop {
                        task_control.dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let _ = task_socket.send_to(&buffer[..size], client).await;
                } else {
                    client_addr = Some(source);
                    let _ = task_socket.send_to(&buffer[..size], host_addr).await;
                }
            }
        });
        Self {
            addr,
            task,
            control,
        }
    }

    fn arm_sparse(&self) {
        self.control
            .mode
            .store(DropMode::Sparse as u8, Ordering::Release);
    }

    fn arm_burst(&self, packets: u64) {
        assert!(packets > 0, "burst must drop at least one packet");
        self.control.burst_left.store(packets, Ordering::Release);
        self.control
            .mode
            .store(DropMode::Burst as u8, Ordering::Release);
    }

    fn dropped_packets(&self) -> u64 {
        self.control.dropped.load(Ordering::Relaxed)
    }

    fn video_packets(&self) -> u64 {
        self.control.video_packets.load(Ordering::Relaxed)
    }
}

impl Drop for DropProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn open_client(addr: SocketAddr, host_fp: [u8; 32]) -> (SessionHandle, EventStream) {
    tokio::time::timeout(
        Duration::from_secs(3),
        rsp_client::Session::open({
            let mut params =
                SessionParams::single_shared(addr, host_fp, Arc::clone(common::device()));
            params.device_name = "loss-proxy-test".into();
            params.start = StartParams {
                max_width: 1280,
                fps: 30,
                bitrate_kbps: 2_000,
                ..StartParams::default()
            };
            params
        }),
    )
    .await
    .expect("session open did not complete")
    .expect("session open failed")
}

#[tokio::test]
async fn fec_recovers_single_datagram_loss_and_picture_continues() {
    let platform = TinyFramePlatform::new();
    let host_fp = platform.host_fingerprint();
    let host = RunningHost::start(platform, host_fp);
    let proxy = DropProxy::start(host.addr).await;
    let (client, mut events) = open_client(proxy.addr, host.host_fp).await;

    let mut frames = 0usize;
    let mut keyframe_count = 0usize;
    let mut frames_after_recovery = 0usize;
    let mut recovery_seen = false;
    let mut armed = false;
    let mut ordered = true;
    let mut last_frame_id: Option<u32> = None;
    let waited = tokio::time::timeout(WAIT_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(ClientEvent::Frame(frame)) => {
                    assert_eq!(
                        frame.frame.data.len(),
                        TINY_FRAME_BYTES,
                        "frame was not shaped as expected"
                    );
                    frames += 1;
                    if !armed && frames >= WARMUP_FRAMES {
                        proxy.arm_sparse();
                        armed = true;
                    }
                    keyframe_count += usize::from(frame.frame.keyframe);
                    if let Some(previous) = last_frame_id {
                        ordered &= frame.frame.frame_id == previous.wrapping_add(1);
                    }
                    last_frame_id = Some(frame.frame.frame_id);
                    if client.stats().frames_recovered > 0 {
                        if recovery_seen {
                            frames_after_recovery += 1;
                        } else {
                            recovery_seen = true;
                        }
                    }
                    if armed
                        && frames >= MIN_FRAMES
                        && proxy.dropped_packets() > 0
                        && recovery_seen
                        && frames_after_recovery >= FRAMES_AFTER_RECOVERY
                    {
                        break;
                    }
                }
                Some(ClientEvent::Error { code, message }) => {
                    panic!("host error {code:?}: {message}");
                }
                Some(ClientEvent::Ended { reason, .. }) => panic!("closed: {reason}"),
                Some(_) => {}
                None => panic!("client event channel closed"),
            }
        }
    })
    .await;
    if waited.is_err() {
        let stats = client.stats();
        panic!(
            "FEC-recoverable loss did not recover within {WAIT_TIMEOUT:?}: \
             frames={frames}, recovered={}, lost={}, requests={}, keyframes={keyframe_count}, \
             dropped={}, video_packets={}, frames_after_recovery={frames_after_recovery}, \
             armed={armed}, parity={}",
            stats.frames_recovered,
            stats.frames_lost,
            stats.keyframe_requests,
            proxy.dropped_packets(),
            proxy.video_packets(),
            fec::parity_count(1, fec::redundancy_ratio(0.0)),
        );
    }

    let stats = client.stats();
    assert!(
        frames >= MIN_FRAMES,
        "expected at least {MIN_FRAMES} frames, got {frames}"
    );
    assert!(
        proxy.dropped_packets() >= 1,
        "proxy did not drop a video datagram"
    );
    assert!(
        stats.frames_recovered >= 1,
        "expected FEC to recover at least one frame: frames={frames}, dropped={}, {stats:?}",
        proxy.dropped_packets()
    );
    assert_eq!(
        stats.frames_lost, 0,
        "single-datagram loss should not lose a frame"
    );
    assert!(
        frames_after_recovery >= FRAMES_AFTER_RECOVERY,
        "picture did not continue after FEC recovery: {frames_after_recovery} frames"
    );
    assert!(ordered, "delivered frame ids were not contiguous");
    client.close();
}

#[tokio::test]
async fn loss_beyond_fec_redundancy_requests_keyframe_and_picture_continues() {
    // The shaped encoder emits one data shard per frame. The initial FEC
    // redundancy is therefore exactly parity_count(1, redundancy_ratio(0)).
    // A burst of 2m+1 consecutive video datagrams is guaranteed to put more
    // than m erasures into one two-datagram frame, regardless of frame-boundary
    // alignment. QUIC encrypts DATAGRAM payloads, so the proxy cannot inspect
    // VideoHeader and instead uses this protocol-level bound.
    let parity = fec::parity_count(1, fec::redundancy_ratio(0.0));
    assert!(parity > 0, "HW12 must enable FEC by default");
    let burst_len = u64::try_from(2 * parity + 1).expect("burst length fits u64");

    let platform = TinyFramePlatform::new();
    let host_fp = platform.host_fingerprint();
    let host = RunningHost::start(platform, host_fp);
    let proxy = DropProxy::start(host.addr).await;
    let (client, mut events) = open_client(proxy.addr, host.host_fp).await;

    let mut frames = 0usize;
    let mut armed = false;
    let mut request_seen = false;
    let mut keyframe_seen = false;
    let mut keyframe_count = 0usize;
    let mut frames_after_keyframe = 0usize;
    let mut ordered_after_keyframe = true;
    let mut last_frame_id: Option<u32> = None;
    let waited = tokio::time::timeout(WAIT_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(ClientEvent::Frame(frame)) => {
                    assert_eq!(
                        frame.frame.data.len(),
                        TINY_FRAME_BYTES,
                        "frame was not shaped as expected"
                    );
                    frames += 1;
                    if !armed && frames >= WARMUP_FRAMES {
                        proxy.arm_burst(burst_len);
                        armed = true;
                    }
                    let stats = client.stats();
                    if !request_seen && stats.keyframe_requests > 0 {
                        request_seen = true;
                    }
                    if request_seen {
                        if keyframe_seen {
                            frames_after_keyframe += 1;
                            if let Some(previous) = last_frame_id {
                                ordered_after_keyframe &=
                                    frame.frame.frame_id == previous.wrapping_add(1);
                            }
                        } else if frame.frame.keyframe {
                            keyframe_seen = true;
                        }
                    }
                    keyframe_count += usize::from(frame.frame.keyframe);
                    last_frame_id = Some(frame.frame.frame_id);
                    if armed
                        && frames >= MIN_FRAMES
                        && proxy.dropped_packets() >= burst_len
                        && request_seen
                        && keyframe_seen
                        && frames_after_keyframe >= FRAMES_AFTER_RECOVERY
                    {
                        break;
                    }
                }
                Some(ClientEvent::Error { code, message }) => {
                    panic!("host error {code:?}: {message}");
                }
                Some(ClientEvent::Ended { reason, .. }) => panic!("closed: {reason}"),
                Some(_) => {}
                None => panic!("client event channel closed"),
            }
        }
    })
    .await;
    if waited.is_err() {
        let stats = client.stats();
        panic!(
            "unrecoverable loss did not request/recover within {WAIT_TIMEOUT:?}: \
             frames={frames}, recovered={}, lost={}, requests={}, keyframes={keyframe_count}, \
             dropped={}, video_packets={}, frames_after_keyframe={frames_after_keyframe}, \
             armed={armed}, parity={parity}, burst={burst_len}",
            stats.frames_recovered,
            stats.frames_lost,
            stats.keyframe_requests,
            proxy.dropped_packets(),
            proxy.video_packets(),
        );
    }

    let stats = client.stats();
    assert!(
        frames >= MIN_FRAMES,
        "expected at least {MIN_FRAMES} frames, got {frames}"
    );
    assert_eq!(
        proxy.dropped_packets(),
        burst_len,
        "burst proxy did not drop exactly the configured datagrams"
    );
    assert!(
        stats.frames_lost >= 1,
        "loss beyond FEC redundancy must lose a frame: {stats:?}"
    );
    assert!(
        stats.keyframe_requests >= 1,
        "expected an automatic KeyframeRequest after unrecoverable loss"
    );
    assert!(
        keyframe_seen,
        "expected a keyframe after the automatic KeyframeRequest"
    );
    assert!(
        frames_after_keyframe >= FRAMES_AFTER_RECOVERY,
        "picture did not continue after the recovery keyframe: {frames_after_keyframe} frames"
    );
    assert!(
        ordered_after_keyframe,
        "delivered frame ids were not contiguous after the recovery keyframe"
    );
    client.close();
}
